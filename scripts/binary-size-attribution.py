import re, sys
from collections import defaultdict

map_path = sys.argv[1]
segs = {}
sec_re = re.compile(r'^\s*([0-9a-fA-F]{4}):([0-9a-fA-F]{8}) ([0-9a-fA-F]+)H (\S+)\s+(CODE|DATA)')
sec_size = defaultdict(int)

def fam(name):
    for p in ('.text', '.rdata', '.xdata', '.pdata', '.data', '.bss'):
        if name == p or name.startswith(p + '$'):
            return p
    return None

sym_re = re.compile(r'^\s*([0-9a-fA-F]{4}):([0-9a-fA-F]{8})\s+(\S+)\s+([0-9a-fA-F]{16})\s+(.+)$')

LTO_BLOB_RE = re.compile(r'^[0-9a-f]{16}-cgu')

def sym_crate(sym):
    m = re.match(r'^_?_?ZN(\d+)', sym)
    if not m:
        # MSVC compiler-generated thunks/constants: ?dtor$N@?0?_ZN..., __xmm@, ??_C@
        m2 = re.search(r'_ZN(\d+)', sym)
        if m2:
            ln = int(m2.group(1))
            start = m2.start() + 3 + len(m2.group(1))
            seg = sym[start:start + ln]
            gm = re.match(r'_\$LT\$([a-z_0-9]+)\.\.', seg)
            return (gm.group(1) if gm else seg) or 'anon'
        if re.match(r'^(__xmm@|__real@|\?\?_C@)', sym):
            return 'msvc-const'
        if sym.startswith('sqlite3'):
            return 'libsqlite3_sys(C)'
        if sym.startswith('git') or sym.startswith('GIT'):
            return 'libgit2(C)'
        return 'C/other'
    ln = int(m.group(1))
    start = sym.index('ZN') + 2 + len(m.group(1))
    return sym[start:start + ln] or 'anon'

def crate_of(obj, sym):
    if 'libgit2_sys' in obj:
        return 'libgit2(C)'
    parts = obj.replace('\\', '/').split('/')[-1].split('.')
    if len(parts) >= 3 and parts[0] == 'nemesisbot':
        if LTO_BLOB_RE.match(parts[2]):
            return sym_crate(sym) + '(lto-blob)'
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

in_secs = False
entries = defaultdict(list)

with open(map_path, encoding='utf-8', errors='replace') as fh:
    for line in fh:
        if not in_secs:
            if line.startswith(' Start         Length     Name'):
                in_secs = True
            continue
        m = sec_re.match(line.rstrip('\n'))
        if m:
            seg, off, ln, name, cls = m.groups()
            f = fam(name)
            if f:
                # first writer wins per seg so .rdata isn't clobbered by .xdata;
                # record sizes per family separately
                sec_size[f] += int(ln, 16)
                segs.setdefault(seg, f)
            continue
        m = sym_re.match(line.rstrip('\n'))
        if m:
            seg, off, sym, rva, obj = m.groups()
            if seg in segs:
                entries[seg].append((int(off, 16), obj, sym))

print('== section sizes ==')
for k, v in sorted(sec_size.items(), key=lambda kv: -kv[1]):
    print(f'{k:8s} {v/1048576:8.2f} MB')

agg = defaultdict(lambda: defaultdict(int))
cnt = defaultdict(lambda: defaultdict(int))
for seg, lst in entries.items():
    f = segs[seg]
    lst.sort()
    uniq = []
    seen = {}
    for off, obj, sym in lst:
        if off in seen:
            continue
        seen[off] = True
        uniq.append((off, obj, sym))
    max_off = uniq[-1][0]
    for i, (off, obj, sym) in enumerate(uniq):
        end = uniq[i + 1][0] if i + 1 < len(uniq) else max_off + 64
        size = end - off
        if size <= 0 or size > 8 * 1024 * 1024:
            continue
        c = crate_of(obj, sym)
        agg[f][c] += size
        cnt[f][c] += 1

for f in ('.text', '.rdata', '.xdata', '.data', '.bss'):
    if f not in agg:
        continue
    tot = sum(agg[f].values())
    print(f'\n== {f}  attributed={tot/1048576:.1f} MB ==')
    items = sorted(agg[f].items(), key=lambda kv: -kv[1])
    for c, v in items:
        print(f'{v/1048576:8.2f} MB  {c:34s} ({cnt[f][c]})')
