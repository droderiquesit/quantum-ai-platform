import pathlib, sys, re

def resolve_union(path):
    """Keep both sides of every conflict, deduped, order preserved (ours then theirs)."""
    p = pathlib.Path(path); lines = p.read_text().split("\n")
    out, i, added = [], 0, 0
    while i < len(lines):
        if lines[i].startswith("<<<<<<<"):
            m = next(j for j in range(i, len(lines)) if lines[j].startswith("======="))
            e = next(j for j in range(m, len(lines)) if lines[j].startswith(">>>>>>>"))
            ours, theirs = lines[i+1:m], lines[m+1:e]
            seen = set(ours)
            extra = [l for l in theirs if l.strip() and l not in seen]
            out.extend(ours + extra); added += len(extra); i = e + 1
        else:
            out.append(lines[i]); i += 1
    p.write_text("\n".join(out))
    return added

for path in sys.argv[1:]:
    n = resolve_union(path)
    print(f"{path}: kept {n} line(s) from the incoming side")
