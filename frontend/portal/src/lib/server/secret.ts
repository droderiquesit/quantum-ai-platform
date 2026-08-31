import { readFileSync } from "node:fs";

/**
 * Reading a credential the deployment supplied, from a variable or a file.
 *
 * This is `qip_core::secret` in TypeScript, deliberately and to the letter.
 * The platform's Rust binaries resolve every credential through that contract;
 * the console is the one process in this system that is not Rust, and a
 * console that resolved its credentials by a different rule would be a second
 * convention for the same job — which is how a deployment ends up with a
 * secret set two ways and a process authenticating with the one nobody thinks
 * is in use.
 *
 * # Why a file at all
 *
 * An environment variable holding a credential is readable from
 * `/proc/<pid>/environ`, is inherited by every child process, and lands in a
 * crash dump. On Cloud Run a secret can be projected as a mounted file
 * instead, which has none of those properties, and
 * `.claude/rules/01-security-and-safety.md` requires the file form wherever
 * the platform can provide it.
 *
 * # The contract, unchanged from the Rust
 *
 * For a credential named `EXAMPLE`:
 *
 * * `EXAMPLE` set — that is the value.
 * * `EXAMPLE_FILE` set — the value is that file's contents.
 * * Neither — `null`. The caller decides whether absence is fatal.
 * * **Both — refused.** Two sources that can disagree is a configuration whose
 *   behaviour depends on which branch happens to be tested first.
 *
 * Trailing whitespace is stripped, because every editor and every `echo` adds
 * a newline and a token that fails to verify for one invisible byte is a bad
 * afternoon. A file that exists and is empty is refused rather than returned:
 * callers that read `null` as "this feature is off" would otherwise turn a
 * missing secret into a disabled control.
 *
 * Nothing here puts a credential in a message. Every error names the variable
 * and never the value.
 */

/** The suffix naming the file variant of a credential variable. */
export const FILE_SUFFIX = "_FILE";

export class SecretNotResolvable extends Error {}

/**
 * The rule, with both sources passed in rather than read.
 *
 * Separated from {@link secretFromEnvironment} so it can be tested without
 * mutating a process-global environment that another test is also reading.
 */
export function resolveSecret(
  variable: string,
  direct: string | undefined,
  fileName: string | undefined,
): string | null {
  const hasDirect = direct !== undefined && direct.length > 0;
  const hasFile = fileName !== undefined && fileName.trim().length > 0;

  if (hasDirect && hasFile) {
    throw new SecretNotResolvable(
      `${variable} and ${variable}${FILE_SUFFIX} are both set. Two sources for one ` +
        `credential can disagree, and the process would authenticate with whichever this ` +
        `code happened to read first. Unset one.`,
    );
  }

  if (hasDirect) {
    const trimmed = direct.replace(/\s+$/u, "");
    if (trimmed.length === 0) {
      throw new SecretNotResolvable(
        `${variable} is set to whitespace. An empty credential is never what was meant, ` +
          `and treating it as absent would turn a missing secret into a disabled control.`,
      );
    }
    return trimmed;
  }

  if (!hasFile) return null;

  let contents: string;
  try {
    contents = readFileSync(fileName.trim(), "utf8");
  } catch (cause) {
    // The path is named — it is configuration, not a secret — and the
    // underlying reason is preserved. The contents never appear.
    throw new SecretNotResolvable(
      `${variable}${FILE_SUFFIX} names ${fileName.trim()}, which could not be read: ` +
        `${cause instanceof Error ? cause.message : "unknown error"}`,
    );
  }

  const trimmed = contents.replace(/\s+$/u, "");
  if (trimmed.length === 0) {
    throw new SecretNotResolvable(
      `${variable}${FILE_SUFFIX} names ${fileName.trim()}, which is empty. A credential ` +
        `file that exists and holds nothing is a projection that has not happened yet, ` +
        `not a feature that is switched off.`,
    );
  }
  return trimmed;
}

/** Resolve `variable`, or the file `variable_FILE` names. */
export function secretFromEnvironment(variable: string): string | null {
  return resolveSecret(
    variable,
    process.env[variable],
    process.env[`${variable}${FILE_SUFFIX}`],
  );
}
