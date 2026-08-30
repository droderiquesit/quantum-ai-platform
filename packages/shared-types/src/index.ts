/**
 * Types shared across every Algorik surface, and the typed environment
 * configuration each one is built against.
 *
 * The configuration half exists because the brief forbids two specific things
 * that are otherwise easy to do: hard-coding Identity Platform settings into
 * application source, and baking a temporary Google-issued URL in so deeply
 * that moving to `algorik.ai` becomes a source rewrite. Both are prevented the
 * same way — every URL and identifier is read from the environment through
 * this one validated shape, so changing where the platform lives is a
 * configuration change, and a missing value stops the surface rather than
 * silently defaulting to something wrong.
 */

/** Which deployment a surface is talking to. Renders as a colour everywhere. */
export type EnvironmentMode = "local" | "development" | "test" | "stage" | "production";

/**
 * The posture the surfaces display, which is not the same as the environment.
 *
 * `live` exists in the type because the platform can *report* it and an
 * operator must be able to see that it did. Nothing in any Algorik surface
 * selects it — see ADR 0014.
 */
export type TradingPosture = "simulation" | "paper" | "stage" | "live";

export interface IdentityConfig {
  /** Google Cloud project hosting Identity Platform. Public, not a secret. */
  readonly projectId: string;
  /** Browser API key. Public by design; it identifies, it does not authorise. */
  readonly apiKey: string;
  /** Domains Identity Platform will honour a redirect to. */
  readonly authorizedDomains: readonly string[];
  /** OAuth client id for Google sign-in. Public. The secret never leaves GCP. */
  readonly googleClientId: string | null;
  /** Exact redirect URI registered with the provider. Must match to the char. */
  readonly redirectUri: string;
}

export interface AlgorikConfig {
  readonly mode: EnvironmentMode;
  readonly posture: TradingPosture;
  /** Public marketing site. */
  readonly siteUrl: string;
  /** Authenticated customer portal. */
  readonly portalUrl: string;
  /** Administrative surface, behind a separate workforce boundary. */
  readonly adminUrl: string;
  /** The BFF this surface calls. Never the platform's internal address. */
  readonly apiUrl: string;
  readonly identity: IdentityConfig | null;
  readonly flags: Readonly<Record<string, boolean>>;
  /** True when any surface is showing generated data. Drives the label. */
  readonly simulationActive: boolean;
}

export interface ConfigProblem {
  readonly key: string;
  readonly problem: string;
}

/**
 * Reads configuration, refusing rather than guessing.
 *
 * The house rule is "refuse rather than guess": a value silently corrected is
 * a caller bug that survives. A portal that defaulted its API URL to localhost
 * in production would come up healthy and serve nothing, which is worse than
 * failing to start.
 */
export function readConfig(
  source: Readonly<Record<string, string | undefined>>,
): { readonly ok: true; readonly config: AlgorikConfig } | { readonly ok: false; readonly problems: readonly ConfigProblem[] } {
  const problems: ConfigProblem[] = [];

  const url = (key: string): string => {
    const raw = source[key]?.trim();
    if (!raw) {
      problems.push({ key, problem: "is not set, and no default is safe for a URL" });
      return "";
    }
    try {
      // Parsed rather than pattern-matched: "https//app.algorik.ai" passes a
      // careless regex and fails every request afterwards.
      new URL(raw);
      return raw.replace(/\/$/, "");
    } catch {
      problems.push({ key, problem: `is not a URL: ${raw}` });
      return "";
    }
  };

  const mode = (source.ALGORIK_ENV ?? "local").trim() as EnvironmentMode;
  if (!["local", "development", "test", "stage", "production"].includes(mode)) {
    problems.push({ key: "ALGORIK_ENV", problem: `is not a known environment: ${mode}` });
  }

  const posture = (source.ALGORIK_POSTURE ?? "paper").trim() as TradingPosture;
  if (!["simulation", "paper", "stage", "live"].includes(posture)) {
    problems.push({ key: "ALGORIK_POSTURE", problem: `is not a known posture: ${posture}` });
  }

  const siteUrl = url("ALGORIK_SITE_URL");
  const portalUrl = url("ALGORIK_PORTAL_URL");
  const adminUrl = url("ALGORIK_ADMIN_URL");
  const apiUrl = url("ALGORIK_API_URL");

  // Identity is optional: the local adapter needs none of it, which is what
  // lets the whole journey be built before a Google project exists. But a
  // half-configured identity is refused — a project id with no redirect URI
  // produces a redirect loop nobody can diagnose from the browser.
  let identity: IdentityConfig | null = null;
  const projectId = source.ALGORIK_IDENTITY_PROJECT_ID?.trim();
  if (projectId) {
    const apiKey = source.ALGORIK_IDENTITY_API_KEY?.trim();
    const redirectUri = source.ALGORIK_IDENTITY_REDIRECT_URI?.trim();
    if (!apiKey) {
      problems.push({ key: "ALGORIK_IDENTITY_API_KEY", problem: "is required once a project id is set" });
    }
    if (!redirectUri) {
      problems.push({
        key: "ALGORIK_IDENTITY_REDIRECT_URI",
        problem: "is required once a project id is set, and must match the provider exactly",
      });
    }
    identity = {
      projectId,
      apiKey: apiKey ?? "",
      redirectUri: redirectUri ?? "",
      googleClientId: source.ALGORIK_GOOGLE_CLIENT_ID?.trim() || null,
      authorizedDomains: (source.ALGORIK_AUTHORIZED_DOMAINS ?? "")
        .split(",")
        .map((domain) => domain.trim())
        .filter((domain) => domain.length > 0),
    };
  }

  if (problems.length > 0) return { ok: false, problems };

  return {
    ok: true,
    config: {
      mode,
      posture,
      siteUrl,
      portalUrl,
      adminUrl,
      apiUrl,
      identity,
      flags: {},
      simulationActive: posture === "simulation",
    },
  };
}

/** A one-line description of why configuration was refused, for a startup log. */
export function describeProblems(problems: readonly ConfigProblem[]): string {
  return problems.map((problem) => `${problem.key} ${problem.problem}`).join("; ");
}
