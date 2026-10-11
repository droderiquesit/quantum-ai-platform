//! Refusing a file path that could climb out of the directory it names.
//!
//! A store that opens `root/segment.N` or appends to a mirror file trusts
//! that the path it was handed stays where its owner put it. A path carrying
//! a `..` component does not: `root/../../etc/x` names a file outside every
//! root the process was configured with, and an `open` will follow it. The
//! failure this prevents is a path assembled from data — a name off the
//! wire, a configured string — reading or appending to a file nobody meant
//! the process to touch.
//!
//! The check is textual on purpose. A path that is not valid UTF-8 is
//! refused rather than lossily converted, because the converted text would
//! not be the path that gets opened; and `..` is refused anywhere in the
//! text, not only as a whole component, because a check that has to agree
//! with every platform's component parser about what a separator is can be
//! talked out of its answer. A file name such as `a..b` is refused too —
//! nothing here names one, and a refusal costs less than a guess.
//!
//! Pure functions: nothing here touches the file system, so `qip-core`
//! stays free of I/O.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

/// `path`, unchanged, if it carries no `..` and is valid UTF-8; otherwise a
/// refusal naming what to do instead.
///
/// Call this on the path itself immediately before opening it, and open the
/// value it returns: a check on one value followed by an `open` of another
/// checks nothing.
pub fn refuse_traversal(path: &Path) -> Result<PathBuf> {
    let Some(text) = path.to_str() else {
        return Err(Error::invalid(
            "a file path that is not valid UTF-8 was refused; name the file in UTF-8",
        ));
    };
    if text.contains("..") {
        return Err(Error::invalid(format!(
            "the file path {text} carries `..` and could leave the directory it was \
             configured under; name the file without a parent-directory component"
        )));
    }
    Ok(PathBuf::from(text))
}

/// `root` joined with `name`, refusing a `name` that is absolute or that
/// carries `..` — either of which would make the join name a file outside
/// `root`. `Path::join` with an absolute argument *replaces* the root rather
/// than extending it, which is the half of this a `..` check alone misses.
pub fn join_under(root: &Path, name: &str) -> Result<PathBuf> {
    if Path::new(name).is_absolute() || name.starts_with('/') || name.starts_with('\\') {
        return Err(Error::invalid(format!(
            "the file name {name} is absolute and would replace the directory it was meant \
             to sit under; name it relative to that directory"
        )));
    }
    refuse_traversal(&root.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_without_a_parent_component_is_returned_unchanged() {
        let path = Path::new("/var/lib/qip/log/segment.00000000000000000042");
        assert_eq!(refuse_traversal(path).ok().as_deref(), Some(path));
    }

    #[test]
    fn a_parent_directory_component_is_refused() {
        let path = Path::new("/var/lib/qip/log/../../../etc/shadow");
        let refused = refuse_traversal(path);
        assert!(refused.is_err());
        assert!(
            refused
                .err()
                .is_some_and(|error| error.to_string().contains("parent-directory component"))
        );
    }

    #[test]
    fn a_relative_climb_is_refused_as_well_as_an_absolute_one() {
        assert!(refuse_traversal(Path::new("../outside")).is_err());
        assert!(refuse_traversal(Path::new("log/..")).is_err());
    }

    #[test]
    fn a_name_joined_under_a_root_stays_under_it() {
        let root = Path::new("/var/lib/qip/log");
        let joined = join_under(root, "segment.00000000000000000007");
        assert_eq!(
            joined.ok(),
            Some(PathBuf::from(
                "/var/lib/qip/log/segment.00000000000000000007"
            ))
        );
    }

    #[test]
    fn an_absolute_name_is_refused_rather_than_replacing_the_root() {
        // Premise: `Path::join` really does discard the root for an absolute
        // argument, which is the behaviour this refusal exists for.
        // (Through a binding: the lint that flags a literal absolute `join`
        // argument is right everywhere except in the test proving why.)
        let absolute = String::from("/etc/shadow");
        assert_eq!(
            Path::new("/var/lib/qip/log").join(&absolute),
            PathBuf::from("/etc/shadow")
        );
        let refused = join_under(Path::new("/var/lib/qip/log"), &absolute);
        assert!(
            refused
                .err()
                .is_some_and(|error| error.to_string().contains("is absolute"))
        );
    }

    #[test]
    fn a_parent_component_in_a_joined_name_is_refused() {
        let refused = join_under(Path::new("/var/lib/qip/log"), "../wal/generation.1");
        assert!(
            refused
                .err()
                .is_some_and(|error| error.to_string().contains("parent-directory component"))
        );
    }
}
