#!/usr/bin/env bash
# .claude/hooks/session-start.sh — rebuild the owner's session tooling inside
# a Claude Code cloud container, at the exact versions the owner approved.
#
# Why this file exists
# --------------------
# Cloud session containers are recreated. Only the repository clone and the
# base image survive a restart: on 2026-10-16 ~/.claude/settings.json,
# ~/.claude/skills, ~/.claude/plugins/cache, ~/.local/bin and ~/.claude-mem
# were all gone afterwards. Without this hook every new session starts
# without the three tools the owner approved on 2026-10-16 — the caveman
# plugin, the claude-mem plugin and graphify — and the session either spends
# its first minutes reinstalling them by hand or works without them. The
# harness runs a SessionStart hook from the repository's .claude/settings.json
# at session start and caches the container state once the hook completes,
# so the cost is paid once per container rather than once per session.
#
# What is approved, and how this script holds to it
# -------------------------------------------------
# Every third-party artefact below runs as the session user inside every
# session: caveman's hooks on SessionStart, SubagentStart and UserPromptSubmit;
# claude-mem's hooks before and after every tool call; graphify's hook-guard
# before every Bash, Grep, Read and Glob call. A floating install would let an
# upstream push — a compromise or just a breaking change — into every fresh
# container with nobody reviewing it. So each artefact is pinned to the
# commit the owner approved, and a mismatch stops this script rather than
# printing a note:
#
#   caveman     tag v4.0.1   = commit d262ab3882952a59312ff3537cd86d894523c5ab
#   claude-mem  tag v13.40.0 = commit 46e2f759e95a1a21b942c1e116f7f12f902705a3
#   graphifyy   0.9.92 from PyPI, with its whole 30-package closure held to
#               graphify-constraints.txt beside this file and to packages
#               uploaded before GRAPHIFY_EXCLUDE_NEWER
#
# "claude plugin marketplace add" documents no ref option, but its <source>
# argument takes one: owner/repo#ref clones that tag or branch (a bare commit
# is refused). The two plugins live inside their marketplace repositories, so
# pinning the marketplace ref pins the plugin files, and the CLI records the
# commit it installed in ~/.claude/plugins/installed_plugins.json as
# gitCommitSha. A tag can be moved, which is why the commit is the identity
# this script checks, not the tag and not the manifest's version string — two
# different commits of claude-mem both report 13.40.0. The approved claude-mem
# commit is the v13.40.0 tag; the copy first installed by hand on 2026-10-16
# was one commit later (26e839a3, "docs: update CHANGELOG for v13.40.0"), a
# 39-line CHANGELOG.md change and nothing else, verified with git diff --stat.
#
# Migration. A container cached before this pinning holds the marketplaces
# without a ref, and the CLI refuses to add a pinned source over a mismatched
# entry. So a marketplace whose listed ref differs is removed (its plugin
# uninstalled first, keeping the plugin's data) and re-added pinned; one the
# user settings declare but the CLI no longer lists, because its two records
# disagree, is retired the same way; and a plugin whose commit differs is
# reinstalled once from the pinned marketplace. If the commit still differs
# after that, the script fails and says the tag may have moved. Proven on
# 2026-10-17 against this container's own hand-made, unpinned install.
#
# What each step writes, and where (everything under $HOME, nothing in the
# repository), and what it fetches over the network
# ------------------------------------------------------------------------
#   1. claude plugin marketplace add JuliusBrussee/caveman#v4.0.1
#        -> $HOME/.claude/settings.json (extraKnownMarketplaces.caveman) and a
#           clone under $HOME/.claude/plugins/marketplaces/caveman
#           (github.com)
#   2. claude plugin marketplace add thedotmack/claude-mem#v13.40.0
#        -> the same file (extraKnownMarketplaces.thedotmack), clone beside it
#           (github.com)
#   3. claude plugin install caveman@caveman
#        -> $HOME/.claude/settings.json (enabledPlugins) and
#           $HOME/.claude/plugins/cache/caveman/caveman/<version>
#   4. claude plugin install claude-mem@thedotmack
#        -> the same file and
#           $HOME/.claude/plugins/cache/thedotmack/claude-mem/<version>. The
#           CLI then runs `bun install --frozen-lockfile --ignore-scripts`
#           there: 28 npm packages, about 480 MB, from registry.npmjs.org,
#           held by the plugin's committed bun.lock, with no lifecycle
#           scripts. The script checks node_modules exists afterwards,
#           because the CLI reports success even when bun is missing.
#   5. uv tool install graphifyy==<pin> -c graphify-constraints.txt
#      --exclude-newer <date>
#        -> $HOME/.local/bin/graphify and $HOME/.local/bin/graphify-mcp, with
#           the environment under $HOME/.local/share/uv/tools/graphifyy
#           (pypi.org and files.pythonhosted.org). The installed closure is
#           compared with the constraints file afterwards.
#   6. graphify install            (run from $HOME — see below)
#        -> $HOME/.claude/skills/graphify/SKILL.md and a "# graphify" section
#           appended to $HOME/.claude/CLAUDE.md
#   7. graphify claude install     (run from $HOME — see below)
#        -> two PreToolUse hooks ("graphify hook-guard search" on Bash|Grep,
#           "graphify hook-guard read" on Read|Glob) merged into
#           $HOME/.claude/settings.json. It also writes its ten-line "##
#           graphify" block to $HOME/CLAUDE.md, the working directory's
#           project file. That file is removed below only when this run
#           created it and it is byte-identical to the block graphify ships,
#           so a CLAUDE.md a person put there is never touched.
#   8. node scripts/ensure-launcher.cjs, run inside the claude-mem plugin
#      cache with CLAUDE_PLUGIN_ROOT pointing at it
#        -> $HOME/.local/bin/claude-mem, compiled with bun. The plugin's own
#           SessionStart hook runs the same script and so heals this itself;
#           running it here only pre-warms the cached container.
#   9. a telemetry decision for claude-mem
#        -> $HOME/.claude-mem/telemetry.json with "enabled": false, written
#           only when no decision is recorded there yet. See below.
#
# Plugins installed during a SessionStart hook are not active in the session
# that installed them: the CLI says "Plugins changed. Run /reload-plugins to
# activate." The script prints that reminder whenever step 3 or 4 installed
# something, so the first session on a fresh container is not mistaken for
# one with the tools live.
#
# What claude-mem does once installed, so the approval is of the whole thing
# ------------------------------------------------------------------------
# Read from the installed 13.40.0 bundle on 2026-10-17. Its PostToolUse hook
# records every tool's input and output — Bash output and file reads
# included — into $HOME/.claude-mem/claude-mem.db, in plaintext, mode 0644,
# which the cached container keeps; a tool result holding a credential is
# therefore persisted. Each observation is also summarised by a model call.
# Its SessionStart hook replays stored observations into later sessions, so
# text an agent read as untrusted data in one session returns as context in
# the next. Its worker listens on 127.0.0.1:37700 with no credential
# (CLAUDE_MEM_WORKER_INGEST_TOKEN is empty). Its usage telemetry defaults ON,
# to us.i.posthog.com, with error stack traces, unless DO_NOT_TRACK is set,
# CLAUDE_MEM_TELEMETRY is "off", or $HOME/.claude-mem/telemetry.json records
# "enabled": false. Cloud sync (cmem.ai) is off: CLOUD_SYNC_TOKEN and
# CLOUD_SYNC_HUB_URL are empty. Of these, only the telemetry is switched
# here, to off, because nobody opted into it and this repository's rules
# treat every safety default as the restrictive one; the repository's
# .claude/settings.json sets CLAUDE_MEM_TELEMETRY=off for every session as
# well, so the decision holds even where this file is not read. The rest is
# what the owner asked for when asking for claude-mem, and is written here so
# that what was approved is what runs.
#
# Why graphify's two installers run from $HOME
# --------------------------------------------
# Both write into the current directory's .claude/settings.json and CLAUDE.md.
# Run from the repository they would edit the committed .claude/settings.json
# and CLAUDE.md — protected paths that only a person's review may change, and
# the file this very hook is registered in. $HOME is outside every checkout,
# so from there they reach the user-level files they are meant for. The last
# thing this script does is prove the repository's settings file and its
# CLAUDE.md are byte-identical to what they were before the first step ran.
#
# Why "claude-mem start" is not here
# ----------------------------------
# Starting the worker is the claude-mem plugin's own SessionStart hook's job,
# which it does asynchronously under its own timeout. Run in the foreground
# the command never returns, and it once filled /tmp with its log. Do not add
# it here and do not add it to any test of this script.
#
# Time and budget. On 2026-10-17 a fresh run took 28 s on a fast proxy and
# 3 min 52 s on a slow one, of which 3 min 33 s was uv downloading graphify's
# closure. The hook's timeout in .claude/settings.json is 900 s. A run the
# harness kills leaves nothing half-done that the next run does not redo:
# every step checks its own marker before acting.
#
# Non-interactive and fail-closed. Every claude invocation reads stdin from
# /dev/null and no -y is passed: a plugin whose marketplace one day declares
# an install-time command is refused by the CLI rather than accepted on
# nobody's say-so, and the failure here names the step.
#
# Idempotent. Every step checks its own marker before doing anything, so a
# start on a cached container costs a few list commands and nothing else.

set -euo pipefail

# Local developers keep their own setup; only a cloud session runs this.
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

: "${HOME:?HOME must be set}"

# --- The approved artefacts --------------------------------------------------

readonly CAVEMAN_MARKETPLACE="caveman"
readonly CAVEMAN_SOURCE="JuliusBrussee/caveman"
readonly CAVEMAN_REF="v4.0.1"
readonly CAVEMAN_PLUGIN="caveman@caveman"
readonly CAVEMAN_VERSION="4.0.1"
readonly CAVEMAN_COMMIT="d262ab3882952a59312ff3537cd86d894523c5ab"

readonly CLAUDE_MEM_MARKETPLACE="thedotmack"
readonly CLAUDE_MEM_SOURCE="thedotmack/claude-mem"
readonly CLAUDE_MEM_REF="v13.40.0"
readonly CLAUDE_MEM_PLUGIN="claude-mem@thedotmack"
readonly CLAUDE_MEM_VERSION="13.40.0"
readonly CLAUDE_MEM_COMMIT="46e2f759e95a1a21b942c1e116f7f12f902705a3"

readonly GRAPHIFY_PIN="0.9.92"
# graphifyy 0.9.92 was uploaded to PyPI at 2026-10-16T17:57:56Z. Nothing
# uploaded after this instant is a candidate for any package in the closure.
readonly GRAPHIFY_EXCLUDE_NEWER="2026-10-17T00:00:00Z"

# uv, graphify and the claude-mem launcher all live in $HOME/.local/bin, and
# a fresh container's PATH is not guaranteed to carry it yet.
case ":${PATH}:" in
  *":${HOME}/.local/bin:"*) ;;
  *) export PATH="${HOME}/.local/bin:${PATH}" ;;
esac

# The repository root: what the harness passes, else derived from this file's
# own location, so a hand run from any directory finds the same file.
repo_dir="${CLAUDE_PROJECT_DIR:-}"
if [ -z "$repo_dir" ]; then
  repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fi
readonly repo_dir
readonly repo_settings="${repo_dir}/.claude/settings.json"
readonly repo_claude_md="${repo_dir}/CLAUDE.md"
readonly graphify_constraints="${repo_dir}/.claude/hooks/graphify-constraints.txt"
readonly config_dir="${HOME}/.claude"
readonly user_settings="${config_dir}/settings.json"
readonly user_claude_md="${config_dir}/CLAUDE.md"
readonly graphify_skill="${config_dir}/skills/graphify/SKILL.md"
readonly installed_plugins="${config_dir}/plugins/installed_plugins.json"
readonly launcher="${HOME}/.local/bin/claude-mem"
readonly claude_mem_data="${CLAUDE_MEM_DATA_DIR:-${HOME}/.claude-mem}"
readonly log="${config_dir}/session-start.log"

mkdir -p "$config_dir"
: >"$log"

# Failure reporting. `step` names what is running; `fail` reports a failure
# this script diagnosed itself, and the EXIT trap reports any other non-zero
# exit under `set -e` so that no failure leaves without naming its step. An
# ERR trap would fire twice for a failure inside a subshell or a command
# substitution, which is why EXIT is used instead.
step="startup"
reported=0
fail() {
  reported=1
  echo "session setup FAILED at ${step}: $* (log: ${log})" >&2
  exit 1
}
on_exit() {
  local code=$?
  if [ "$code" -ne 0 ] && [ "$reported" -eq 0 ]; then
    echo "session setup FAILED at ${step}: exit status ${code} (log: ${log})" >&2
  fi
}
trap on_exit EXIT

say() { echo "session-start: ${step}: $*"; }

# Tool output goes to the log rather than into the session's context, and
# stdin is closed so nothing can wait on a prompt.
run() { "$@" >>"$log" 2>&1 </dev/null; }

fingerprint() { # $1 = file; "absent" when there is none
  if [ -f "$1" ]; then
    sha256sum "$1" | cut -d' ' -f1
  else
    echo "absent"
  fi
}

for tool in claude jq node bun sha256sum cmp; do
  command -v "$tool" >/dev/null 2>&1 || fail "'${tool}' is not on PATH"
done

# graphify's installers and the plugin CLI both resolve the user's Claude
# directory from $HOME; a CLAUDE_CONFIG_DIR pointing elsewhere would split the
# writes between two trees, so it is refused rather than guessed around.
if [ -n "${CLAUDE_CONFIG_DIR:-}" ] && [ "${CLAUDE_CONFIG_DIR%/}" != "$config_dir" ]; then
  fail "CLAUDE_CONFIG_DIR is '${CLAUDE_CONFIG_DIR}', not '${config_dir}'; this script only knows the default layout"
fi

[ -f "$graphify_constraints" ] || fail "${graphify_constraints} is missing, so graphify's closure cannot be held"

settings_before="$(fingerprint "$repo_settings")"
claude_md_before="$(fingerprint "$repo_claude_md")"
installed_this_run=0

# --- Marker probes ----------------------------------------------------------

marketplace_present() { # $1 = marketplace name as `claude plugin marketplace list` reports it
  claude plugin marketplace list --json </dev/null 2>>"$log" \
    | jq -e --arg name "$1" 'any(.[]; .name == $name)' >/dev/null
}

marketplace_ref() { # $1 = marketplace name; prints the ref the CLI lists for it, or nothing
  claude plugin marketplace list --json </dev/null 2>>"$log" \
    | jq -r --arg name "$1" '.[] | select(.name == $name) | .ref // empty'
}

# The CLI keeps two records of a marketplace — the declaration in the user
# settings and ~/.claude/plugins/known_marketplaces.json — and lists nothing
# when they disagree, while still refusing to add over the declaration. A
# declared-but-unlisted marketplace is that half-written state.
marketplace_declared() { # $1 = marketplace name
  [ -f "$user_settings" ] && jq -e --arg name "$1" '
    (.extraKnownMarketplaces // {}) | has($name)
  ' "$user_settings" >/dev/null 2>>"$log"
}

plugin_present() { # $1 = plugin id, e.g. caveman@caveman
  claude plugin list --json </dev/null 2>>"$log" \
    | jq -e --arg id "$1" 'any(.[]; .id == $id)' >/dev/null
}

plugin_field() { # $1 = plugin id, $2 = field of its `claude plugin list --json` entry
  claude plugin list --json </dev/null 2>>"$log" \
    | jq -r --arg id "$1" --arg field "$2" '.[] | select(.id == $id) | .[$field] // empty'
}

plugin_commit() { # $1 = plugin id; prints the commit the CLI installed, or nothing
  [ -f "$installed_plugins" ] || return 0
  jq -r --arg id "$1" '
    (.plugins[$id] // []) | map(select(.scope == "user")) | .[0].gitCommitSha // empty
  ' "$installed_plugins" 2>>"$log"
}

graphify_version() { graphify --version 2>/dev/null | awk '{print $2}'; }

# The interpreter of graphify's own environment, for reading what it ships
# and what it resolved. Never run from a directory the installer wrote.
graphify_python() {
  local bin
  bin="$(readlink -f "$(command -v graphify)")" || return 1
  printf '%s/python' "$(dirname "$bin")"
}

graphify_skill_present() {
  [ -f "$graphify_skill" ] && [ -f "$user_claude_md" ] && grep -q '^# graphify' "$user_claude_md"
}

user_hooks_present() {
  [ -f "$user_settings" ] && jq -e '
    [ .hooks.PreToolUse // [] | .[] | .hooks[]? | .command // "" ] as $commands
    | any($commands[]; test("graphify hook-guard search$"))
      and any($commands[]; test("graphify hook-guard read$"))
  ' "$user_settings" >/dev/null 2>>"$log"
}

# --- Steps ------------------------------------------------------------------

# Take a marketplace and its plugin out so the pinned source can go in. The
# plugin is uninstalled first with --keep-data, because `marketplace remove`
# on its own cascades to the plugins it served and deletes their saved data
# "where it can"; claude-mem's observations are not something a re-pin may
# throw away.
retire_marketplace() { # $1 = name, $2 = plugin id served from it
  if plugin_present "$2"; then
    run claude plugin uninstall "$2" --keep-data --json
    plugin_present "$2" && fail "plugin '$2' is still installed after uninstall, so marketplace '$1' cannot be re-pinned"
  fi
  run claude plugin marketplace remove "$1" --json
  marketplace_present "$1" && fail "marketplace '$1' is still listed after remove"
  marketplace_declared "$1" && fail "marketplace '$1' is still declared in ${user_settings} after remove"
  return 0
}

ensure_marketplace() { # $1 = name, $2 = owner/repo, $3 = ref, $4 = plugin id served from it
  local have
  if marketplace_present "$1"; then
    have="$(marketplace_ref "$1")"
    if [ "$have" = "$3" ]; then
      say "skipped, marketplace '$1' already at $2#$3"
      return 0
    fi
    # A container cached before the pin, or a different pin: the CLI refuses
    # to add a pinned source over a mismatched entry, so the entry goes, and
    # its plugin with it (reinstalled under the pin by the next step).
    say "migrating marketplace '$1' from ref '${have:-none}' to $3"
    retire_marketplace "$1" "$4"
  elif marketplace_declared "$1"; then
    say "marketplace '$1' is declared in ${user_settings} but the CLI does not list it (its two records disagree); retiring the declaration before adding it pinned"
    retire_marketplace "$1" "$4"
  fi
  run claude plugin marketplace add "$2#$3" --json
  marketplace_present "$1" || fail "marketplace '$1' still absent after add"
  have="$(marketplace_ref "$1")"
  [ "$have" = "$3" ] || fail "marketplace '$1' records ref '${have:-none}' after add, expected $3"
  say "done, added marketplace '$1' from $2#$3"
}

ensure_plugin() { # $1 = plugin id, $2 = approved version string, $3 = approved commit
  local have_commit have_version reinstalled=0 did_install=0
  while :; do
    if plugin_present "$1"; then
      have_commit="$(plugin_commit "$1")"
      have_version="$(plugin_field "$1" version)"
      if [ "$have_commit" = "$3" ]; then
        if [ "$did_install" -eq 0 ]; then
          say "skipped, plugin '$1' already at ${have_version} (commit ${3:0:12}, approved)"
        else
          say "done, installed plugin '$1' at ${have_version} (commit ${3:0:12}, approved)"
        fi
        if [ "$have_version" != "$2" ]; then
          say "note, '$1' reports version ${have_version} where ${2} was approved; the commit matches, so the files are the approved ones and only the manifest string differs"
        fi
        return 0
      fi
      if [ "$reinstalled" -ne 0 ]; then
        fail "plugin '$1' is at commit '${have_commit:-unknown}' after a reinstall from the pinned marketplace, expected ${3}; the tag may have been moved upstream, and nothing here accepts that silently"
      fi
      say "plugin '$1' is at commit '${have_commit:-unknown}', expected ${3}; reinstalling it from the pinned marketplace"
      run claude plugin uninstall "$1" --keep-data --json
      plugin_present "$1" && fail "plugin '$1' is still installed after uninstall"
      reinstalled=1
    fi
    run claude plugin install "$1" --json
    plugin_present "$1" || fail "plugin '$1' still absent after install"
    did_install=1
    installed_this_run=1
  done
}

step="step 1 (marketplace ${CAVEMAN_MARKETPLACE})"
ensure_marketplace "$CAVEMAN_MARKETPLACE" "$CAVEMAN_SOURCE" "$CAVEMAN_REF" "$CAVEMAN_PLUGIN"

step="step 2 (marketplace ${CLAUDE_MEM_MARKETPLACE})"
ensure_marketplace "$CLAUDE_MEM_MARKETPLACE" "$CLAUDE_MEM_SOURCE" "$CLAUDE_MEM_REF" "$CLAUDE_MEM_PLUGIN"

step="step 3 (plugin ${CAVEMAN_PLUGIN})"
ensure_plugin "$CAVEMAN_PLUGIN" "$CAVEMAN_VERSION" "$CAVEMAN_COMMIT"

step="step 4 (plugin ${CLAUDE_MEM_PLUGIN})"
ensure_plugin "$CLAUDE_MEM_PLUGIN" "$CLAUDE_MEM_VERSION" "$CLAUDE_MEM_COMMIT"
# The CLI prints success for this plugin even when bun was absent and its 28
# npm packages never arrived; the directory is the only proof they did.
claude_mem_root="$(plugin_field "$CLAUDE_MEM_PLUGIN" installPath)"
[ -n "$claude_mem_root" ] || fail "the CLI reports no installPath for ${CLAUDE_MEM_PLUGIN}"
[ -d "${claude_mem_root}/node_modules" ] \
  || fail "${claude_mem_root}/node_modules is missing: 'bun install --frozen-lockfile' did not run or failed (read the log)"

step="step 5 (graphify ${GRAPHIFY_PIN})"
if command -v graphify >/dev/null 2>&1 && [ "$(graphify_version)" = "$GRAPHIFY_PIN" ]; then
  say "skipped, graphify ${GRAPHIFY_PIN} already installed"
else
  command -v uv >/dev/null 2>&1 || fail "'uv' is not on PATH, so graphify cannot be installed"
  if command -v graphify >/dev/null 2>&1; then
    say "graphify $(graphify_version) is present where ${GRAPHIFY_PIN} is approved; reinstalling to the pin"
  fi
  run uv tool install "graphifyy==${GRAPHIFY_PIN}" \
    --constraints "$graphify_constraints" \
    --exclude-newer "$GRAPHIFY_EXCLUDE_NEWER"
  command -v graphify >/dev/null 2>&1 || fail "graphify is not on PATH after install"
  [ "$(graphify_version)" = "$GRAPHIFY_PIN" ] || fail "graphify reports $(graphify_version) after installing ${GRAPHIFY_PIN}"
  say "done, installed graphify ${GRAPHIFY_PIN}"
fi
# What uv resolved must be exactly what the constraints file names: a package
# the file does not mention, or at another version, is a closure nobody
# reviewed. The comparison ignores the file's comments and blank lines.
python_bin="$(graphify_python)" || fail "cannot locate graphify's own interpreter"
resolved="$(uv pip freeze --python "$python_bin" 2>>"$log" | sort)"
expected="$(grep -v -E '^\s*(#|$)' "$graphify_constraints" | tr -d ' ' | sort)"
if [ "$resolved" != "$expected" ]; then
  {
    echo "resolved closure:"; echo "$resolved"
    echo "constraints file:"; echo "$expected"
  } >>"$log"
  fail "graphify's installed closure differs from ${graphify_constraints} (both are listed in the log)"
fi
say "ok, graphify's $(printf '%s\n' "$expected" | wc -l | tr -d ' ')-package closure matches the constraints file"

step="step 6 (graphify skill and user CLAUDE.md section)"
if graphify_skill_present; then
  say "skipped, ${graphify_skill} and the graphify section of ${user_claude_md} exist"
else
  (cd "$HOME" && run graphify install)
  graphify_skill_present || fail "${graphify_skill} or the graphify section of ${user_claude_md} is missing after install"
  say "done, wrote ${graphify_skill} and the graphify section of ${user_claude_md}"
fi

step="step 7 (graphify hook-guard hooks in user settings)"
if user_hooks_present; then
  say "skipped, both graphify hook-guard hooks already in ${user_settings}"
else
  # The installer also writes its "## graphify" block to the working
  # directory's CLAUDE.md, here $HOME/CLAUDE.md. Remove that only when this
  # run created it and it is byte-identical to the block graphify ships; a
  # file that was already there, or that holds anything else, is left alone.
  stray="${HOME}/CLAUDE.md"
  stray_existed=0
  [ -e "$stray" ] && stray_existed=1
  (cd "$HOME" && run graphify claude install)
  user_hooks_present || fail "the graphify hook-guard hooks are missing from ${user_settings} after install"
  say "done, registered both graphify hook-guard hooks in ${user_settings}"
  if [ "$stray_existed" -eq 0 ] && [ -f "$stray" ]; then
    block="$("$python_bin" -I -c 'import graphify, pathlib; print(pathlib.Path(graphify.__file__).parent / "always_on" / "claude-md.md")' 2>>"$log")" || block=""
    if [ -n "$block" ] && [ -f "$block" ] && cmp -s "$stray" "$block"; then
      rm -f "$stray"
      say "removed ${stray}, the installer's own ten-line block that this run created"
    else
      say "left ${stray} as found: this run created it but it is not the installer's block"
    fi
  fi
fi

step="step 8 (claude-mem launcher)"
if [ -x "$launcher" ]; then
  say "skipped, ${launcher} already present"
else
  if [ ! -f "${claude_mem_root}/scripts/ensure-launcher.cjs" ]; then
    fail "the claude-mem plugin cache has no scripts/ensure-launcher.cjs (installPath: '${claude_mem_root}')"
  fi
  (cd "$claude_mem_root" && run env CLAUDE_PLUGIN_ROOT="$claude_mem_root" node scripts/ensure-launcher.cjs)
  # ensure-launcher exits 0 even when it could not compile, so the file is
  # the only proof.
  [ -x "$launcher" ] || fail "${launcher} was not produced; ensure-launcher exits 0 on failure, read the log"
  say "done, compiled ${launcher}"
fi

step="step 9 (claude-mem telemetry decision)"
telemetry_file="${claude_mem_data}/telemetry.json"
if [ -f "$telemetry_file" ] && jq -e '.enabled == true or .enabled == false' "$telemetry_file" >/dev/null 2>>"$log"; then
  say "skipped, ${telemetry_file} already records enabled=$(jq -r '.enabled' "$telemetry_file")"
else
  mkdir -p "$claude_mem_data"
  now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  if [ -f "$telemetry_file" ] && jq -e 'type == "object"' "$telemetry_file" >/dev/null 2>>"$log"; then
    # Keep the installId the worker already minted; only the decision is new.
    jq --arg now "$now" '. + {enabled: false, decidedAt: $now}' "$telemetry_file" >"${telemetry_file}.tmp"
  else
    install_id="$(node -e 'process.stdout.write(require("crypto").randomUUID())')"
    jq -n --arg id "$install_id" --arg now "$now" '{installId: $id, enabled: false, decidedAt: $now}' >"${telemetry_file}.tmp"
  fi
  mv "${telemetry_file}.tmp" "$telemetry_file"
  jq -e '.enabled == false' "$telemetry_file" >/dev/null || fail "${telemetry_file} does not record enabled=false after writing it"
  say "done, recorded enabled=false in ${telemetry_file} (no one opted into usage telemetry)"
fi

step="final check (repository files untouched)"
settings_after="$(fingerprint "$repo_settings")"
claude_md_after="$(fingerprint "$repo_claude_md")"
if [ "$settings_before" != "$settings_after" ]; then
  fail "${repo_settings} changed while this script ran (sha256 ${settings_before} -> ${settings_after}); either a step wrote into the repository or another process edited it meanwhile — re-run once before blaming the hook"
fi
if [ "$claude_md_before" != "$claude_md_after" ]; then
  fail "${repo_claude_md} changed while this script ran (sha256 ${claude_md_before} -> ${claude_md_after}); either a step wrote into the repository or another process edited it meanwhile — re-run once before blaming the hook"
fi
say "ok, ${repo_settings} and ${repo_claude_md} unchanged (sha256 ${settings_after}, ${claude_md_after})"

if [ "$installed_this_run" -ne 0 ]; then
  echo "session-start: note, a plugin was installed in this run; it is active after /reload-plugins or the next session start"
fi
echo "session setup complete"
