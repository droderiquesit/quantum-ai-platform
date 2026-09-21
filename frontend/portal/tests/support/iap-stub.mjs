import { createServer } from "node:http";
import { generateKeyPairSync, sign as signRaw } from "node:crypto";

/**
 * A stand-in for Google's IAP signing service, for the `iap.spec.ts` suite.
 *
 * The console verifies `x-goog-iap-jwt-assertion` against a published key set.
 * Testing that for real needs two things this process provides and nothing
 * else in the suite can: a key set the console will fetch, and the ability to
 * mint assertions against it — including assertions that are wrong in one
 * specific way each.
 *
 * **No key is committed.** Both keypairs are generated at start-up and live
 * only in this process's memory for the length of a run. A fixed test key in
 * the repository would be a private key in a committed file, which is the
 * thing the standing rules refuse regardless of what it is for, and it would
 * also be a key somebody could eventually point a real deployment at.
 *
 * Two keypairs, and the second is the point of the first:
 *
 * - **trusted** — published at `/jwks`. The console will import it.
 * - **rogue** — never published, and `/mint?key=rogue` signs with it *under
 *   the trusted key's `kid`*. That shape is deliberate: a verifier that looks
 *   up a key by `kid` and forgets to check the signature accepts this token,
 *   and a verifier that checks the signature rejects it. A rogue token with
 *   its own `kid` would be rejected by the lookup alone and would prove
 *   nothing about whether any cryptography ran.
 *
 * The console reaches this on 127.0.0.1, which is the only host its
 * `ALGORIK_IAP_JWKS_URL` override accepts — see `lib/server/iap.ts` for why an
 * override that could name any host would be a way to hand the console's trust
 * root to whoever can set one environment variable.
 */

const PORT = Number(process.env.PORT ?? 3318);
const TRUSTED_KID = "algorik-test-trusted";

const trusted = generateKeyPairSync("ec", { namedCurve: "P-256" });
const rogue = generateKeyPairSync("ec", { namedCurve: "P-256" });

const publishedJwk = { ...trusted.publicKey.export({ format: "jwk" }), kid: TRUSTED_KID, alg: "ES256", use: "sig" };

function base64url(value) {
  return Buffer.from(value, "utf8").toString("base64url");
}

/**
 * Mint one assertion to order.
 *
 * Every parameter exists because one test needs it wrong. `alg` so a test can
 * present `none`; `key` so a test can forge; `aud` so a test can present a
 * token minted for a different backend service — which is a *genuine*
 * Google-signed token in production and the reason the audience check is not
 * optional.
 */
function mint(query) {
  const nowSeconds = Math.floor(Date.now() / 1000);
  const lifetimeSeconds = Number(query.get("ttl") ?? 600);
  const algorithm = query.get("alg") ?? "ES256";
  const header = { alg: algorithm, kid: query.get("kid") ?? TRUSTED_KID, typ: "JWT" };
  const claims = {
    iss: query.get("iss") ?? "https://cloud.google.com/iap",
    aud: query.get("aud") ?? "",
    sub: query.get("sub") ?? "accounts.google.com:000000000000000000000",
    email: query.get("email") ?? "",
    iat: nowSeconds - 5,
    exp: nowSeconds + lifetimeSeconds,
  };
  const hostedDomain = query.get("hd");
  if (hostedDomain) claims.hd = hostedDomain;

  const signingInput = `${base64url(JSON.stringify(header))}.${base64url(JSON.stringify(claims))}`;
  // Note what an `alg` override does *not* do: it does not drop the signature.
  // A `none` token with an empty signature is refused by any length check and
  // proves nothing about whether the algorithm was ever looked at. This mints
  // the dangerous shape instead — a header claiming `none` over a genuine
  // ES256 signature by the published key — so the only thing that can refuse
  // it is a verifier that picks its algorithm from its own configuration
  // rather than from the token.
  const key = query.get("key") === "rogue" ? rogue.privateKey : trusted.privateKey;
  const signature = signRaw("sha256", Buffer.from(signingInput, "utf8"), {
    key,
    dsaEncoding: "ieee-p1363",
  });
  return `${signingInput}.${signature.toString("base64url")}`;
}

const server = createServer((request, response) => {
  const url = new URL(request.url ?? "/", `http://127.0.0.1:${PORT}`);
  if (url.pathname === "/jwks") {
    response.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" });
    response.end(JSON.stringify({ keys: [publishedJwk] }));
    return;
  }
  if (url.pathname === "/mint") {
    response.writeHead(200, { "content-type": "text/plain", "cache-control": "no-store" });
    response.end(mint(url.searchParams));
    return;
  }
  response.writeHead(404, { "content-type": "text/plain" });
  response.end("not found");
});

server.listen(PORT, "127.0.0.1");
