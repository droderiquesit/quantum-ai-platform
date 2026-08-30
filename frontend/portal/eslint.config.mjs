import { defineConfig, globalIgnores } from "eslint/config";
import nextVitals from "eslint-config-next/core-web-vitals";
import nextTs from "eslint-config-next/typescript";

const eslintConfig = defineConfig([
  ...nextVitals,
  ...nextTs,
  // Override default ignores of eslint-config-next.
  globalIgnores([
    // Default ignores of eslint-config-next:
    ".next/**",
    "out/**",
    "build/**",
    "next-env.d.ts",
    // Vendored licensed template packages. They are third-party source kept
    // as the visual reference and their own build output — linting them
    // reports thousands of findings nobody can act on without editing a
    // licensed artefact, which would drown the findings that matter.
    "admin/**",
    "landing/**",
    "mobile/**",
    "logos/**",
  ]),
]);

export default eslintConfig;
