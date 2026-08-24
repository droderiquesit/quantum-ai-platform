//! A [`crate::BlobStore`] over a Cloud Storage bucket.
//!
//! The four operations of the port map onto four calls of the Cloud Storage
//! JSON API: a media upload, a media download, an object delete, and a prefix
//! listing. Nothing else is in the mapping, and the type documentation below
//! says what that leaves out.

use super::{GcpAccess, percent_encode, status_refusal};
use crate::blob::BlobStore;
use qip_core::error::{Error, Result};
use qip_transport::{HttpClient, Method};
use serde::Deserialize;

/// Cloud Storage object names are capped at this many bytes of UTF-8.
///
/// Google's limit, enforced here so an over-long key is refused where the
/// caller can see which key it was, rather than by a 400 whose body names the
/// request instead of the object.
const MAX_KEY_BYTES: usize = 1024;

/// Objects one listing page asks for. Google's own maximum is 1000.
const DEFAULT_PAGE_SIZE: u32 = 1000;

/// How many listing pages [`CloudStorageBlobStore::list`] will follow.
///
/// A bound rather than "until the tokens stop", because a listing is the one
/// operation here whose cost is set by the bucket rather than by the caller: a
/// prefix matching ten million objects would otherwise be ten thousand
/// sequential round trips holding a growing `Vec`, on the calling thread.
const DEFAULT_MAX_LIST_PAGES: u32 = 100;

/// Everything a deployment must decide before a bucket can be used.
#[derive(Clone, Debug)]
pub struct CloudStorageConfig {
    /// The bucket. No default: a wrong bucket that happened to exist would be
    /// written to successfully, which is the failure this crate exists to
    /// avoid.
    pub bucket: String,
    /// Largest object this adapter will upload in one request. See
    /// [`CloudStorageBlobStore`] on why there is no resumable upload.
    pub max_object_bytes: usize,
    /// Objects per listing page.
    pub page_size: u32,
    /// Listing pages followed before the operation is refused.
    pub max_list_pages: u32,
    /// Key prefix every object in this store lives under.
    ///
    /// One bucket holds many namespaces, so without a prefix two of them both
    /// writing `model.bin` would be one object and the second write would
    /// destroy the first. Empty means the store owns the whole bucket, which is
    /// only correct when nothing else writes to it.
    pub prefix: String,
    /// `content-type` stamped on every uploaded object.
    ///
    /// One value for the whole store because [`BlobStore::put`] carries bytes
    /// and no type. `application/octet-stream` is the honest default: it says
    /// "bytes", which is exactly what the port promises, instead of asserting a
    /// format the store cannot know.
    pub content_type: String,
    /// Endpoint and credential.
    pub access: GcpAccess,
}

impl CloudStorageConfig {
    /// A configuration naming a bucket and nothing else, so that
    /// [`CloudStorageBlobStore::missing_configuration`] can report the rest.
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
            // Matched to the default `ClientLimits::max_body`, so an object
            // this adapter is willing to upload is one it can also download.
            // Two different numbers here would produce a store that accepts
            // writes it cannot read back.
            max_object_bytes: 32 * 1024 * 1024,
            page_size: DEFAULT_PAGE_SIZE,
            max_list_pages: DEFAULT_MAX_LIST_PAGES,
            prefix: String::new(),
            content_type: "application/octet-stream".into(),
            access: GcpAccess::unconfigured(),
        }
    }

    /// Read the bucket and access from the environment.
    ///
    /// `QIP_CLOUD_STORAGE_BUCKET` names the bucket; the endpoint and
    /// credential come from [`GcpAccess::from_env`]. An unset bucket is an
    /// error rather than a default, because a default that happened to name a
    /// real bucket would be written to successfully and nobody would find out
    /// until they went looking for the archive somewhere else.
    pub fn from_env(clock: std::sync::Arc<dyn qip_core::Clock>) -> Result<Self> {
        let bucket = std::env::var(super::BUCKET_VARIABLE)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                Error::unavailable(format!(
                    "no Cloud Storage bucket: set {}. There is no default, because a default \
                     naming a real bucket would be written to successfully",
                    super::BUCKET_VARIABLE
                ))
            })?;
        Ok(Self::new(bucket).with_access(GcpAccess::from_env(clock)?))
    }

    pub fn with_access(mut self, access: GcpAccess) -> Self {
        self.access = access;
        self
    }

    pub fn with_max_object_bytes(mut self, bytes: usize) -> Self {
        self.max_object_bytes = bytes;
        self
    }

    pub fn with_max_list_pages(mut self, pages: u32) -> Self {
        self.max_list_pages = pages;
        self
    }

    pub fn with_page_size(mut self, objects: u32) -> Self {
        self.page_size = objects;
        self
    }

    /// Confine this store to a prefix within the bucket.
    ///
    /// Trailing slashes are trimmed so that `archive` and `archive/` name the
    /// same scope: two stores that differed only in a trailing slash would
    /// silently be two different namespaces.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into().trim_matches('/').to_string();
        self
    }

    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = content_type.into();
        self
    }
}

/// Blob storage backed by a Cloud Storage bucket.
///
/// # What it is for
///
/// The things [`crate::provider::StorageTarget::CloudStorage`] was always
/// meant to hold: event-log archives, model artifacts, reports — objects
/// written once, read rarely, and kept because somebody may later have to
/// justify a decision from them.
///
/// # What it does not promise
///
/// **No resumable upload.** A `put` is one `uploadType=media` request carrying
/// the whole object, so an upload that fails has to be repeated from the
/// beginning and an object larger than [`CloudStorageConfig::max_object_bytes`]
/// is refused before a connection is opened. Resumable upload is a session
/// protocol — initiate, chunk, commit, resume after failure — and implementing
/// it without exercising it against real interruptions would produce something
/// that looks like durability and is not. The bound is the honest version.
///
/// **No streaming.** `get` buffers the whole object in memory, bounded by
/// [`qip_transport::ClientLimits::max_body`]. An object past that limit is
/// refused with [`qip_core::Error::Guard`] rather than truncated. A truncated
/// archive that reported success is precisely the failure this refusal exists
/// to prevent — it would be discovered years later by whoever needed the
/// archive.
///
/// **`digest` is not Google's checksum.** The port's [`BlobStore::digest`] is
/// SHA-256 of the bytes, and Cloud Storage computes MD5 and CRC32C, not
/// SHA-256. This adapter therefore does *not* override `digest`: it downloads
/// the object and hashes it locally, like every other adapter. Returning
/// Google's `md5Hash` instead would be cheaper and would silently mean that
/// the same blob has two different digests depending on which store held it,
/// which would make an integrity check between them impossible to trust.
///
/// **No consistency claims beyond Google's.** This adapter adds no caching and
/// no read-after-write logic of its own; what it reports is what the API said.
///
/// **No conditional writes.** A `put` overwrites whatever is at the key. The
/// port has no compare-and-set, so adding generation preconditions here would
/// be a guarantee only one adapter could keep.
///
/// # Keys
///
/// `put`, `get` and `delete` refuse keys that [`crate::FileBlobStore`] would
/// also refuse — empty ones, ones beginning with `/`, and ones with a `.`,
/// `..` or empty path segment — even though Cloud Storage itself would accept
/// some of them. The reason is portability: a namespace written through this
/// adapter must be readable through the file-backed one, and a key that only
/// works on one of them turns a change of storage target into data loss.
///
/// `list` does *not* filter: it returns every name the API reports, including
/// any that this adapter would refuse to fetch. Hiding those would make an
/// object that exists invisible, which is a worse failure than an explicit
/// refusal when something tries to read it.
#[derive(Debug)]
pub struct CloudStorageBlobStore {
    config: CloudStorageConfig,
    client: HttpClient,
}

impl CloudStorageBlobStore {
    /// What a deployment must supply beyond a working configuration.
    ///
    /// These stand even when every field is set, which is why the provider's
    /// requirement text is never empty for this target. A configured adapter is
    /// not by itself a production archive.
    pub const REQUIREMENTS: [&'static str; 4] = [
        "a TLS-terminating proxy at the configured endpoint: `qip_transport::http` has no TLS \
         stack and refuses `https` by name rather than downgrading it, so a bearer token sent \
         straight to `storage.googleapis.com` would cross the internet in clear text",
        "a bearer token from a `TokenSource` the deployment keeps fresh — this crate cannot mint \
         one, because that means RS256-signing a JWT and ADR 0009 forbids in-tree cryptography",
        "a service account holding `roles/storage.objectAdmin` on the bucket, or the narrower \
         object-level roles for whichever of put/get/delete/list this deployment actually uses",
        "a lifecycle policy and a retention decision on the bucket: this adapter deletes exactly \
         what it is asked to and never expires anything, so an archive's retention is the \
         bucket's configuration and not this code's behaviour",
    ];

    /// Build the store. Succeeds even when nothing is configured: an adapter
    /// that cannot reach a bucket still has to exist in order to say why.
    ///
    /// Fails only on configuration that is present and wrong — an empty bucket
    /// name, a zero page size, a limit that would refuse every object.
    pub fn new(config: CloudStorageConfig) -> Result<Self> {
        if config.bucket.trim().is_empty() {
            return Err(Error::invalid(
                "a Cloud Storage blob store needs a bucket name: there is no default, because a \
                 default that happened to name a real bucket would be written to successfully",
            ));
        }
        if !config
            .bucket
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        {
            return Err(Error::invalid(format!(
                "the bucket name {:?} contains a character this adapter will not put in a request \
                 line: Cloud Storage bucket names are ASCII letters, digits, and . - _",
                config.bucket
            )));
        }
        if !config.prefix.is_empty() {
            // The prefix goes into every key, so a prefix that is not itself a
            // usable key would make every operation fail with an error about
            // the caller's key rather than about the configuration.
            if config.prefix.chars().any(char::is_control)
                || config
                    .prefix
                    .split('/')
                    .any(|part| part == ".." || part == "." || part.is_empty())
            {
                return Err(Error::invalid(format!(
                    "unsafe Cloud Storage key prefix: {}. It is prepended to every key in this \
                     store, so it has to be a usable key itself",
                    config.prefix
                )));
            }
        }
        if config.max_object_bytes == 0 {
            return Err(Error::invalid(
                "max_object_bytes is zero, which would refuse every upload including an empty one",
            ));
        }
        if config.page_size == 0 {
            return Err(Error::invalid(
                "page_size is zero: a listing that asks for no objects per page never terminates",
            ));
        }
        if config.max_list_pages == 0 {
            return Err(Error::invalid(
                "max_list_pages is zero, which would refuse every listing",
            ));
        }
        if config
            .content_type
            .chars()
            .any(|c| c.is_control() || c == '\r' || c == '\n')
        {
            return Err(Error::invalid(
                "the content type contains a control character; in a header value it would end \
                 the header and let the rest be read as another",
            ));
        }
        let client = HttpClient::new(config.access.limits());
        Ok(Self { config, client })
    }

    pub fn config(&self) -> &CloudStorageConfig {
        &self.config
    }

    /// Whether this store can reach its bucket at all.
    pub fn is_available(&self) -> bool {
        self.config.access.is_configured()
    }

    /// Configuration a deployment has not supplied, each named on its own.
    pub fn missing_configuration(&self) -> Vec<String> {
        self.config.access.missing_configuration()
    }

    /// The full text of what production must supply: what is missing now,
    /// followed by what is required even when nothing is.
    pub fn requirement(&self) -> String {
        let mut parts = self.missing_configuration();
        parts.extend(Self::REQUIREMENTS.iter().map(|r| (*r).to_string()));
        parts.join("; ")
    }

    /// The refusal every entry point returns when the store cannot be reached.
    ///
    /// Never a fallback to local disk. A deployment pointed at a bucket that
    /// quietly wrote to a container filesystem would report every write as a
    /// success, pass every smoke test, and lose the archive when the pod was
    /// rescheduled.
    fn unavailable(&self) -> Error {
        Error::unavailable(format!(
            "the Cloud Storage blob store for bucket {} cannot reach it and will not write \
             anywhere else: {}",
            self.config.bucket,
            self.requirement()
        ))
    }

    fn service(&self) -> String {
        format!("Cloud Storage bucket {}", self.config.bucket)
    }

    /// Refuse a key that is not portable across this crate's blob adapters.
    ///
    /// See the type documentation: the set refused here is
    /// [`crate::FileBlobStore`]'s, plus what would break the request itself.
    fn validate_key(&self, key: &str) -> Result<()> {
        if key.is_empty() {
            return Err(Error::invalid(
                "an empty object name: Cloud Storage has no such object and never will",
            ));
        }
        if key.len() > MAX_KEY_BYTES {
            return Err(Error::invalid(format!(
                "the object name is {} bytes and Cloud Storage's limit is {MAX_KEY_BYTES}",
                key.len()
            )));
        }
        if key.chars().any(char::is_control) {
            return Err(Error::invalid(format!(
                "the object name {key:?} contains a control character, which cannot go in a \
                 request line"
            )));
        }
        if key.starts_with('/')
            || key
                .split('/')
                .any(|part| part == ".." || part == "." || part.is_empty())
        {
            return Err(Error::invalid(format!(
                "unsafe blob key: {key}. This adapter refuses the keys the file-backed adapter \
                 refuses, so that a namespace written to a bucket can still be read back from \
                 disk; Cloud Storage itself would accept this one"
            )));
        }
        Ok(())
    }

    /// The object name a caller's key becomes.
    ///
    /// Applied after validation, so the error a bad key produces names the key
    /// the caller passed rather than the prefixed one they have never seen.
    fn scoped(&self, key: &str) -> String {
        if self.config.prefix.is_empty() {
            key.to_string()
        } else {
            format!("{}/{key}", self.config.prefix)
        }
    }

    /// A name the API reported, with this store's prefix removed.
    ///
    /// An object under the queried prefix that does not start with it cannot
    /// happen against Cloud Storage, so it is treated as the proxy or the peer
    /// answering about a different bucket — refused rather than returned under
    /// a name that would not fetch.
    fn unscoped(&self, name: &str) -> Result<String> {
        if self.config.prefix.is_empty() {
            return Ok(name.to_string());
        }
        let head = format!("{}/", self.config.prefix);
        name.strip_prefix(&head).map(str::to_string).ok_or_else(|| {
            Error::schema(format!(
                "{} listed the object {name:?}, which is not under this store's prefix {:?}. It \
                 is not returned: a name that would not fetch is worse than a listing that \
                 refuses",
                self.service(),
                self.config.prefix
            ))
        })
    }

    /// `/storage/v1/b/{bucket}/o/{object}` — the object resource.
    fn object_path(&self, key: &str) -> String {
        format!(
            "/storage/v1/b/{}/o/{}",
            percent_encode(&self.config.bucket),
            percent_encode(&self.scoped(key))
        )
    }
}

impl BlobStore for CloudStorageBlobStore {
    /// Upload the whole object in one request.
    ///
    /// `uploadType=media` rather than `multipart`: the port carries no metadata
    /// to send alongside the bytes, so the simpler form is the honest one.
    fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        if !self.is_available() {
            return Err(self.unavailable());
        }
        self.validate_key(key)?;
        if bytes.len() > self.config.max_object_bytes {
            return Err(Error::guard(format!(
                "the object is {} bytes and this store uploads at most {} in one request: there \
                 is no resumable upload here, so a larger object is refused rather than started \
                 and abandoned",
                bytes.len(),
                self.config.max_object_bytes
            )));
        }
        let path = format!(
            "/upload/storage/v1/b/{}/o?uploadType=media&name={}",
            percent_encode(&self.config.bucket),
            percent_encode(&self.scoped(key))
        );
        let request = self
            .config
            .access
            .request(Method::Post, &path)?
            .with_header("content-type", &self.config.content_type)
            .with_body(bytes);
        let response = self.client.send(&request).map_err(Error::from)?;
        if !response.is_success() {
            return Err(status_refusal(
                &self.service(),
                response.status,
                &response.body_excerpt(),
            ));
        }
        Ok(())
    }

    /// Download the whole object, or report that there is none.
    ///
    /// A 404 is `Ok(None)` because "no such blob" is the port's normal answer,
    /// not a failure. Every other non-2xx is an error: a 403 that returned
    /// `None` would make a permissions problem look like an empty archive.
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        if !self.is_available() {
            return Err(self.unavailable());
        }
        self.validate_key(key)?;
        let path = format!("{}?alt=media", self.object_path(key));
        let request = self.config.access.request(Method::Get, &path)?;
        let response = self.client.send(&request).map_err(Error::from)?;
        if response.status == 404 {
            return Ok(None);
        }
        if !response.is_success() {
            return Err(status_refusal(
                &self.service(),
                response.status,
                &response.body_excerpt(),
            ));
        }
        Ok(Some(response.body))
    }

    /// Delete the object, reporting whether there was one.
    ///
    /// A 404 is `Ok(false)` rather than an error, matching the port: deleting
    /// something that is not there is not a failure. Cloud Storage answers a
    /// successful delete with 204 and no body.
    fn delete(&self, key: &str) -> Result<bool> {
        if !self.is_available() {
            return Err(self.unavailable());
        }
        self.validate_key(key)?;
        let request = self
            .config
            .access
            .request(Method::Delete, &self.object_path(key))?;
        let response = self.client.send(&request).map_err(Error::from)?;
        if response.status == 404 {
            return Ok(false);
        }
        if !response.is_success() {
            return Err(status_refusal(
                &self.service(),
                response.status,
                &response.body_excerpt(),
            ));
        }
        Ok(true)
    }

    /// Every object name under `prefix`, in the order the API returns them.
    ///
    /// Cloud Storage filters by prefix server-side and lists lexicographically,
    /// which is the order the port promises. Pagination is followed up to
    /// [`CloudStorageConfig::max_list_pages`]; hitting that bound is an error
    /// and not a truncated list, because a caller cannot tell a short list from
    /// a complete one and would act on it as if it were complete.
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        if !self.is_available() {
            return Err(self.unavailable());
        }
        let mut names = Vec::new();
        let mut page_token: Option<String> = None;
        for _ in 0..self.config.max_list_pages {
            let mut path = format!(
                "/storage/v1/b/{}/o?prefix={}&maxResults={}",
                percent_encode(&self.config.bucket),
                percent_encode(&self.scoped(prefix)),
                self.config.page_size
            );
            if let Some(token) = &page_token {
                path.push_str("&pageToken=");
                path.push_str(&percent_encode(token));
            }
            let request = self.config.access.request(Method::Get, &path)?;
            let response = self.client.send(&request).map_err(Error::from)?;
            if !response.is_success() {
                return Err(status_refusal(
                    &self.service(),
                    response.status,
                    &response.body_excerpt(),
                ));
            }
            let body = response.body_as_str().map_err(Error::from)?;
            let page: ObjectListing = serde_json::from_str(body).map_err(|error| {
                Error::schema(format!(
                    "{} sent a listing this decoder cannot read: {error}. The first bytes of it \
                     were: {}",
                    self.service(),
                    response.body_excerpt()
                ))
            })?;
            // Strip the store's own prefix back off, so what comes out of
            // `list` is what would go into `get` — a listing that returned
            // prefixed names would make every round trip through it fail.
            for item in page.items {
                names.push(self.unscoped(&item.name)?);
            }
            match page.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => return Ok(names),
            }
        }
        Err(Error::guard(format!(
            "listing {prefix:?} in {} did not finish within {} pages of {} objects. The list is \
             not returned truncated: a caller cannot tell a short list from a complete one, and \
             would treat the missing objects as absent",
            self.service(),
            self.config.max_list_pages,
            self.config.page_size
        )))
    }
}

/// One page of `objects.list`.
///
/// Unknown fields are ignored — Google adding one is not a fault and must not
/// break a listing — but the fields this decoder reads are read strictly.
#[derive(Debug, Default, Deserialize)]
struct ObjectListing {
    #[serde(default)]
    items: Vec<ObjectResource>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ObjectResource {
    /// The object's name, which is the blob key.
    name: String,
}
