#!/usr/bin/env bash
# compare-image-trees.sh EXCEPTIONS FIRST SECOND
#
# Compares two exported image filesystems file by file (R12-CICD-10117) and
# exits 0 only when they agree everywhere outside the paths EXCEPTIONS names.
# deploy.yml runs this on the portal's two builds; the acceptance suite
# (qip-acceptance/tests/deploy_provenance.rs) runs this same file on trees it
# builds, so the logic the pipeline trusts is the logic that was tested.
#
# EXCEPTIONS holds one absolute image path per line; blank lines and lines
# starting with `#` are ignored. FIRST and SECOND are directories holding the
# two images' root filesystems.
#
# Refused, each one named on stderr:
#   - a path in one tree and not the other (added or missing);
#   - a path whose type, permission bits, symlink target or bytes differ;
#   - a listed exception that is not a regular file in both trees. A listed
#     file gone means the image changed shape under the list, and the list
#     would go on excusing a path that no longer holds what it was excused
#     for;
#   - an exceptions file with a malformed or repeated line.
# Ownership and times are not compared: times are clamped to the commit's
# epoch by the build, and ownership depends on who extracted the archive.
set -euo pipefail
export LC_ALL=C

if [ "$#" -ne 3 ]; then
  echo "usage: $0 EXCEPTIONS FIRST SECOND" >&2
  exit 2
fi
exceptions_file=$1
first=$2
second=$3
for dir in "$first" "$second"; do
  if [ ! -d "$dir" ]; then
    echo "$dir is not a directory; there is no image filesystem to compare." >&2
    exit 2
  fi
done
if [ ! -f "$exceptions_file" ]; then
  echo "the exceptions list $exceptions_file does not exist." >&2
  exit 2
fi

declare -A excepted=()
while IFS= read -r line || [ -n "$line" ]; do
  # Trim surrounding whitespace; only whole-line comments are comments.
  line="${line#"${line%%[![:space:]]*}"}"
  line="${line%"${line##*[![:space:]]}"}"
  case "$line" in
    '' | '#'*) continue ;;
    /*) ;;
    *)
      echo "$exceptions_file: '$line' is not an absolute image path." >&2
      exit 2
      ;;
  esac
  case "$line/" in
    */../* | */./* | *//*)
      echo "$exceptions_file: '$line' is not a plain path." >&2
      exit 2
      ;;
  esac
  if [ -n "${excepted[$line]:-}" ]; then
    echo "$exceptions_file: '$line' is listed twice." >&2
    exit 2
  fi
  excepted[$line]=1
done < "$exceptions_file"

problems=()

for path in "${!excepted[@]}"; do
  for which in first second; do
    root=$first
    [ "$which" = first ] || root=$second
    if [ -L "$root$path" ] || [ ! -f "$root$path" ]; then
      problems+=("listed exception $path is not a file in the $which build")
    fi
  done
done

listing() {
  (cd "$1" && find . -mindepth 1 -printf '/%P\0' | sort -z)
}
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
listing "$first" > "$scratch/first"
listing "$second" > "$scratch/second"
comm -z -23 "$scratch/first" "$scratch/second" > "$scratch/only-first"
comm -z -13 "$scratch/first" "$scratch/second" > "$scratch/only-second"
comm -z -12 "$scratch/first" "$scratch/second" > "$scratch/both"

while IFS= read -r -d '' path; do
  [ -n "${excepted[$path]:-}" ] || problems+=("$path is in the first build and missing from the second")
done < "$scratch/only-first"
while IFS= read -r -d '' path; do
  [ -n "${excepted[$path]:-}" ] || problems+=("$path is in the second build and missing from the first")
done < "$scratch/only-second"

compared=0
while IFS= read -r -d '' path; do
  [ -z "${excepted[$path]:-}" ] || continue
  compared=$((compared + 1))
  a="$(stat -c '%F %a' "$first$path")"
  b="$(stat -c '%F %a' "$second$path")"
  if [ "$a" != "$b" ]; then
    problems+=("$path differs in type or mode: $a against $b")
  elif [ -L "$first$path" ]; then
    if [ "$(readlink "$first$path")" != "$(readlink "$second$path")" ]; then
      problems+=("$path is a symlink to two different targets")
    fi
  elif [ -f "$first$path" ] && ! cmp -s "$first$path" "$second$path"; then
    problems+=("$path differs between the two builds")
  fi
done < "$scratch/both"

# Two empty trees agree about everything. An export that produced nothing is
# a broken comparison, not a reproducible build.
if [ "$compared" -eq 0 ]; then
  problems+=("no path outside the exceptions is in both builds; the comparison compared nothing")
fi

if [ "${#problems[@]}" -ne 0 ]; then
  printf '%s\n' "${problems[@]}" | sort >&2
  echo "the two builds differ outside the ${#excepted[@]} paths $exceptions_file excuses (${#problems[@]} finding(s)); nothing is pushed." >&2
  echo "Find what moved. Lengthening the list is a person's decision, made in review, never a way to clear this." >&2
  exit 1
fi
echo "the two builds agree file by file: ${compared} paths compared, the ${#excepted[@]} paths $exceptions_file excuses left out."
