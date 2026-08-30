/**
 * Typed feature flags.
 *
 * Every flag is declared here with a default and a reason, so a flag can never
 * be read that nobody declared, and a flag left on by accident is visible in
 * one file rather than inferred from its call sites.
 *
 * Defaults are the safe answer. A flag whose default enables something is a
 * flag that enables it in every environment where configuration failed to
 * load — which is the environment where you least want it on.
 */

export interface FlagDefinition {
  readonly description: string;
  /** The value when configuration says nothing. Always the safe one. */
  readonly fallback: boolean;
}

export const FLAGS = {
  signUpEnabled: {
    description: "Whether new accounts may be created from the landing site.",
    fallback: false,
  },
  googleSignIn: {
    description: "Google sign-in. Requires a configured OAuth client.",
    fallback: false,
  },
  passkeys: {
    description: "WebAuthn passkeys as an authentication method.",
    fallback: false,
  },
  enterpriseSso: {
    description: "SAML and OIDC for organizations.",
    fallback: false,
  },
  partnerPortal: { description: "The partner workspace.", fallback: false },
  developerPortal: { description: "API keys and developer documentation.", fallback: false },
  kycOnboarding: {
    description:
      "Identity, AML and suitability onboarding. Off until a vendor and a legal position exist; the platform must never appear to perform regulatory onboarding it has not implemented.",
    fallback: false,
  },
} as const;

export type FlagName = keyof typeof FLAGS;

export type FlagValues = { readonly [K in FlagName]: boolean };

/**
 * Resolves flags from a configuration record, ignoring anything undeclared.
 *
 * An unknown key is dropped rather than passed through: a typo'd flag name
 * that silently resolved would read as "the feature is off" forever, and the
 * person who set it would be certain they had turned it on.
 */
export function resolveFlags(source: Readonly<Record<string, unknown>> = {}): FlagValues {
  const resolved = {} as { -readonly [K in FlagName]: boolean };
  for (const name of Object.keys(FLAGS) as FlagName[]) {
    const raw = source[name];
    resolved[name] = typeof raw === "boolean" ? raw : raw === "true" ? true : FLAGS[name].fallback;
  }
  return resolved;
}

/** Names of flags that are on, for a diagnostics panel. */
export function enabledFlags(values: FlagValues): readonly FlagName[] {
  return (Object.keys(values) as FlagName[]).filter((name) => values[name]);
}
