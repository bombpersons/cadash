"""Cadash object (enemy/NPC/item) table parser.  See docs/re/objects.md.

All addresses are ROM addresses.  Usage:  python3 extract/objtables.py [rooms|scripts|anims]
"""
import os, sys, struct
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lzss import rom
r = rom()
def W(a): return struct.unpack('>H', r[a:a+2])[0]
def S(a): return struct.unpack('>h', r[a:a+2])[0]
def L(a): return struct.unpack('>I', r[a:a+4])[0]

NROOMS = 48
T_MARKERS   = 0x1C6D6   # [room] -> (x,y,id) byte triples, 0xFF end    (decoded by 0x13C32)
T_PMARKERS  = 0x1CAFA   # [room] -> (x,y,id,flag) byte quads, 0xFF end  (decoded by 0x14B12)
T_RECORDS   = 0x1CCAC   # [room] -> [idx] -> spawn record (normal)       (0x13E80/0x14060)
T_PRECORDS  = 0x1CD70   # [room] -> [idx] -> spawn record (persistent)   (0x13F68/0x1410A)
T_TYPES     = 0x1EEC2   # [room] -> [type] -> [state] -> state script     (0x141AE)
T_ANIMS     = 0x21EEE   # [room] -> [anim id] -> anim descriptor          (0x1421A)
T_BANKS     = 0x2BA98   # [room] -> sprite graphics bank list
T_PAL2      = 0x4A0A    # [room] -> 16 words palette line 2
T_PAL3      = 0x4C8A    # [room] -> 16 words palette line 3
BEHAVIOURS  = 0x14C08   # 135 entries, called every frame  (script byte 1)
CONDITIONS  = 0x14E24   # 175 entries, return d6!=0 => goto state (script pairs)
INITS       = 0x150E0   # 239 entries, called once on state entry (script byte 2)
N_BEH, N_COND, N_INIT = (CONDITIONS-BEHAVIOURS)//4, (INITS-CONDITIONS)//4, 239

REC_FIELDS = ['type', 'cost', 'hp', 'exp', 'gold', 'death_type', 'defence', 'flags',
              'touch_dmg', 'attack_dmg', 'tile_ofs', 'param118']

def table_bounds(base):
    """sorted unique per-room pointers of a [room] table"""
    return sorted(set(L(base + 4*i) for i in range(NROOMS)))

def markers(room):
    a = L(T_MARKERS + 4*room); out = []
    while r[a] != 0xFF:
        x, y, i = r[a], r[a+1], r[a+2]; a += 3
        out.append(dict(cx=x, cy=y, id=i, persistent=bool(i & 0x40) and not (i & 0x80),
                        high_pool=bool(i & 0xA0), index=i & 0x0F))
    return out

def pmarkers(room):
    a = L(T_PMARKERS + 4*room); out = []
    while r[a] != 0xFF:
        x, y, i, f = r[a], r[a+1], r[a+2], r[a+3]; a += 4
        # 0x14B12: bit7 set -> normal table (no 0x8000), else persistent table
        out.append(dict(cx=x, cy=y, id=i, guard_flag=f, persistent=not (i & 0x80),
                        high_pool=bool(i & 0x20), index=i & 0x0F))
    return out

def _list(base, room):
    bounds = table_bounds(base) + [0x1D390 if base == T_RECORDS else 0x1D390]
    allb = sorted(set(table_bounds(T_RECORDS) + table_bounds(T_PRECORDS)))
    a = L(base + 4*room)
    nxt = min([b for b in allb if b > a] + [a + 64*4])
    out = []
    q = a
    while q < nxt:
        p = L(q)
        if not (0x1D390 <= p < 0x1EEC2): break
        out.append(p); q += 4
    return out

def record(p):
    d = {k: W(p + 2*i) for i, k in enumerate(REC_FIELDS)}
    d['addr'] = p
    return d

def records(room, persistent=False):
    return [record(p) for p in _list(T_PRECORDS if persistent else T_RECORDS, room)]

def type_table(room): return L(T_TYPES + 4*room)

def type_count(tt):
    """number of type entries in a type table (entries point to state lists in 0x1F66A..0x205B9)"""
    tts = set(L(T_TYPES + 4*i) for i in range(NROOMS)); t = 0
    while True:
        p = L(tt + 4*t)
        if not (0x1EEC2 <= p < 0x205BA) or (t and tt + 4*t in tts): return t
        t += 1
def anim_table(room): return L(T_ANIMS + 4*room)

def state_script(p):
    """state script: anim, behaviour, init, then (cond, next_state)* 0xFF"""
    anim, beh, init = r[p], r[p+1], r[p+2]
    conds = []; q = p + 3
    while r[q] != 0xFF:
        conds.append((r[q], r[q+1])); q += 2
    return dict(addr=p, anim=anim, behaviour=beh, init=init, conds=conds)

def type_states(states_ptr, max_states=64):
    """follow state graph from state 0; returns {state: script}"""
    out = {}; todo = [0]
    while todo:
        s = todo.pop()
        if s in out or s >= max_states: continue
        if not (0x1EEC2 <= states_ptr + 4*s < 0x21EEE): continue
        p = L(states_ptr + 4*s)
        if not (0x20000 <= p < 0x21EEE): continue
        sc = state_script(p); out[s] = sc
        for c, n in sc['conds']: todo.append(n)
    return dict(sorted(out.items()))

def anim_desc(p):
    w, h, last, loop = W(p), W(p+2), W(p+4), W(p+6)
    frames = [dict(duration=W(p+8+6*k), mapping=L(p+10+6*k)) for k in range(last+1)]
    return dict(addr=p, coll_w=w, coll_h=h, last=last, loop=loop, frames=frames)

def anim_count(table):
    tabs = sorted(set(L(T_ANIMS + 4*i) for i in range(NROOMS)))
    n = 0
    while True:
        a = table + 4*n
        if a in tabs and n: break
        p = L(a)
        if not (0x22000 <= p < 0x2BA98) or p in tabs: break
        n += 1
        if n > 256: break
    return n

def mapping(mp):
    n = W(mp); hdr = [S(mp+2+2*i) for i in range(8)]
    pcs = []
    for k in range(n):
        q = mp + 18 + 8*k
        if W(q) == 0x8000: continue
        pcs.append(dict(dx=S(q), dy=S(q+2), size=W(q+4) & 0xFF, attr=W(q+6)))
    return dict(hurt=dict(w=hdr[0], h=hdr[1], dx=hdr[2], dy=hdr[3]),
                attack=dict(w=hdr[4], h=hdr[5], dx=hdr[6], dy=hdr[7]), pieces=pcs)

def types_used(room):
    """type ids referenced by this room's spawn records (incl. death types and drops)"""
    s = set()
    for pers in (False, True):
        for rec in records(room, pers):
            s.add(rec['type'])
            if rec['death_type'] not in (0xFF, 0xFFFF): s.add(rec['death_type'])
            g = rec['gold']
            if g & 0x80: s.add(g & 0xF)
    s.add(2)
    return s

if __name__ == '__main__':
    cmd = sys.argv[1] if len(sys.argv) > 1 else 'rooms'
    rooms = [int(x) for x in sys.argv[2:]] or range(NROOMS)
    if cmd == 'rooms':
        for room in rooms:
            print('room', room, 'types', hex(type_table(room)), 'anims', hex(anim_table(room)))
            for m in markers(room): print('  M', m)
            for m in pmarkers(room): print('  P', m)
            for i, rec in enumerate(records(room)): print('  R%d' % i, {k: (hex(v) if k in ('addr', 'flags') else v) for k, v in rec.items()})
            for i, rec in enumerate(records(room, True)): print('  PR%d' % i, {k: (hex(v) if k in ('addr', 'flags') else v) for k, v in rec.items()})
    elif cmd == 'scripts':
        done = set()
        for room in rooms:
            tt = type_table(room)
            for t in sorted(types_used(room)):
                sp = L(tt + 4*t)
                if (tt, t) in done: continue
                done.add((tt, t))
                print('table %x type %d (0x%x) states @%x' % (tt, t, t, sp))
                for s, sc in type_states(sp).items():
                    print('   S%-2d @%x anim=%d beh=%d init=%d  %s' % (s, sc['addr'], sc['anim'], sc['behaviour'], sc['init'],
                          ' '.join('C%d->%d' % c for c in sc['conds'])))
