import re, sys
from collections import defaultdict

# Per-module drill-down for the three biggest own-code crates, from the /MAP file.
# Usage: python nb_drill.py <map> <crate-name> ...
# Parses .text symbols; attributes LTO-blob + thunk symbols by embedded mangled name;
# bucket = 2nd mangled path segment (1st is crate), for handlers/commands goes one deeper.

map_path = sys.argv[1]
targets = set(sys.argv[2:])

sec_re = re.compile(r'^\s*([0-9a-fA-F]{4}):([0-9a-fA-F]{8}) ([0-9a-fA-F]+)H (\S+)\s+(CODE|DATA)')
sym_re = re.compile(r'^\s*([0-9a-fA-F]{4}):([0-9a-fA-F]{8})\s+(\S+)\s+([0-9a-fA-F]{16})\s+(.+)$')
LTO_BLOB_RE = re.compile(r'^[0-9a-f]{16}-cgu')

segs = {}
text_seg = None
entries = defaultdict(list)

def mangle_segments(sym):
    # extract leading length-prefixed segments from a legacy-mangled name
    m = re.search(r'_ZN', sym)
    if not m:
        return None
    pos = m.end()
    segs_out = []
    while pos < len(sym):
        m2 = re.match(r'(\d+)', sym[pos:])
        if not m2:
            break
        ln = int(m2.group(1))
        if ln == 0 or ln > 200:
            break
        seg = sym[pos + len(m2.group(1)): pos + len(m2.group(1)) + ln]
        if len(seg) < ln:
            break
        if seg.startswith('_$LT$'):
            break  # generic segment: stop
        segs_out.append(seg)
        pos += len(m2.group(1)) + ln
        if len(segs_out) > 6:
            break
    return segs_out or None

def crate_of(obj, sym):
    parts = obj.replace('\\', '/').split('/')[-1].split('.')
    if len(parts) >= 3 and parts[0] == 'nemesisbot':
        if LTO_BLOB_RE.match(parts[2]):
            return sym_crate(sym)
        return parts[2]
    base = obj.replace('\\', '/').split('/')[-1]
    base = re.sub(r'\.rcgu\.o(\.rcgu\.o)?$', '', base)
    base = re.sub(r'-cgu\.\d+$', '', base)
    base = re.sub(r'-[0-9a-f]{16}$', '', base)
    if ':' in base:
        base = base.split(':')[-1]
    if '(' in base:
        base = base.split('(')[-1].rstrip(')')
    return re.sub(r'\.o$', '', base) or base

def sym_crate(sym):
    m = re.match(r'^_?_?ZN(\d+)', sym)
    if not m:
        m2 = re.search(r'_ZN(\d+)', sym)
        if m2:
            ln = int(m2.group(1))
            start = m2.start() + 3 + len(m2.group(1))
            seg = sym[start:start + ln]
            return seg or 'anon'
        return 'other'
    ln = int(m.group(1))
    start = sym.index('ZN') + 2 + len(m.group(1))
    return sym[start:start + ln] or 'anon'

def module_of(crate, sym):
    segs_out = mangle_segments(sym)
    if not segs_out or len(segs_out) < 2:
        return '(top-level)'
    if segs_out[0] != crate and not segs_out[0].startswith(crate):
        # impl generics etc: first meaningful segment may be a type path
        pass
    rest = segs_out[1:] if segs_out[0] == crate else segs_out
    if not rest:
        return '(top-level)'
    mod = rest[0]
    if mod in ('handlers', 'commands') and len(rest) >= 2:
        return mod + '::' + rest[1]
    if mod.startswith('_$LT$'):
        return '(generics)'
    return mod

in_secs = False
with open(map_path, encoding='utf-8', errors='replace') as fh:
    for line in fh:
        if not in_secs:
            if line.startswith(' Start         Length     Name'):
                in_secs = True
            continue
        m = sec_re.match(line.rstrip('\n'))
        if m:
            seg, off, ln, name, cls = m.groups()
            if name == '.text' or name.startswith('.text$'):
                segs.setdefault(seg, True)
            continue
        m = sym_re.match(line.rstrip('\n'))
        if m:
            seg, off, sym, rva, obj = m.groups()
            if seg in segs:
                entries[seg].append((int(off, 16), obj, sym))

agg = defaultdict(lambda: defaultdict(int))
cnt = defaultdict(lambda: defaultdict(int))
for seg, lst in entries.items():
    lst.sort()
    uniq, seen = [], set()
    for off, obj, sym in lst:
        if off in seen:
            continue
        seen.add(off)
        uniq.append((off, obj, sym))
    max_off = uniq[-1][0]
    for i, (off, obj, sym) in enumerate(uniq):
        end = uniq[i + 1][0] if i + 1 < len(uniq) else max_off + 64
        size = end - off
        if size <= 0 or size > 8 * 1024 * 1024:
            continue
        c = crate_of(obj, sym)
        base_c = c.replace('(lto-blob)', '')
        if base_c not in targets:
            continue
        mod = module_of(base_c, sym)
        agg[base_c][mod] += size
        cnt[base_c][mod] += 1

for c in targets:
    if c not in agg:
        continue
    tot = sum(agg[c].values())
    print(f'\n== {c} .text drill-down  attributed={tot/1048576:.1f} MB ==')
    for m, v in sorted(agg[c].items(), key=lambda kv: -kv[1]):
        print(f'{v/1048576:8.2f} MB  {m:44s} ({cnt[c][m]})')
