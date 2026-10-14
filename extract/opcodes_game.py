"""Structured object-script opcode table for the game (game/assets/objects/opcodes.ops.json).

Simple opcodes (parameter presets, timer tests, proximity tests) are parsed from the descriptions in
extract/out/objects/opcodes.json; everything else is listed as "special" and implemented in Rust.
"""
import json, os, re
HERE = os.path.dirname(os.path.abspath(__file__))
ops = json.load(open(os.path.join(HERE, 'out', 'objects', 'opcodes.json')))
NUM = r'(-?\d+(?:\.\d+)?)'

def hexnum(s):
    return int(s, 16) if s.startswith('0x') else int(s)

def parse_init(desc):
    """Pure assignment lists: vx/vy/ax/ay/face_left/despawn_timer/sound."""
    # Drop a trailing "[also ...]" note only: descriptions may start with RAM writes like "[FF0B92]=1".
    d = re.sub(r'\s*\[also[^\]]*\]\s*$', '', desc).strip().rstrip(';')
    parts = [p.strip() for p in re.split(r';', d) if p.strip()]
    out = {}
    for p in parts:
        m = re.fullmatch(r'(vx|vy|ax|ay)=' + NUM, p)
        if m: out[m.group(1)] = float(m.group(2)); continue
        m = re.fullmatch(r'face_left=([01])', p)
        if m: out['face_left'] = int(m.group(1)); continue
        m = re.fullmatch(r'despawn_timer=1', p)
        if m: out['despawn_timer'] = 1; continue
        m = re.fullmatch(r'sound\s*\(?(0x[0-9A-Fa-f]+|[0-9A-F]{2})\)?', p)
        if m:
            out['sound'] = int(m.group(1), 16); continue
        m = re.fullmatch(r'(vx|vy|ax|ay)=0', p)
        if m: out[m.group(1)] = 0.0; continue
        if p == 'nop': continue
        return None
    return out

def parse_cond(desc):
    m = re.fullmatch(r'COND state_timer==(\d+)', desc.strip())
    if m: return {'timer_eq': int(m.group(1))}
    m = re.fullmatch(r'COND (some|no) player within \|dx\|<=(0x[0-9A-Fa-f]+|\d+),\|dy\|<=(0x[0-9A-Fa-f]+|\d+|any)', desc.strip())
    if m:
        dy = None if m.group(3) == 'any' else hexnum(m.group(3))
        return {'near': [hexnum(m.group(2)), dy], 'negate': m.group(1) == 'no'}
    m = re.fullmatch(r'(right|left) & near\((0x[0-9A-Fa-f]+),(0x[0-9A-Fa-f]+)\)', desc.strip())
    if m:
        return {'near': [hexnum(m.group(2)), hexnum(m.group(3))], 'side': m.group(1)}
    return None

out = {'init': {}, 'cond': {}, 'special': {}}
stats = {'I': [0, 0], 'C': [0, 0], 'B': [0, 0]}
for k, v in ops.items():
    kind, n = k[0], k[1:]
    desc = v['desc']
    if kind == 'I':
        p = parse_init(desc)
        if p is not None: out['init'][n] = p; stats['I'][0] += 1; continue
    if kind == 'C':
        p = parse_cond(desc)
        if p is not None: out['cond'][n] = p; stats['C'][0] += 1; continue
    stats[kind][1] += 1
    out['special'][k] = desc
# ROM tables used by boss scripts (written by extract/boss_tables.py if present).
tables_path = os.path.join(HERE, 'out', 'objects', 'tables.json')
out['tables'] = json.load(open(tables_path)) if os.path.exists(tables_path) else {}
os.makedirs(os.path.join(HERE, '..', 'game', 'assets', 'objects'), exist_ok=True)
json.dump(out, open(os.path.join(HERE, '..', 'game', 'assets', 'objects', 'opcodes.ops.json'), 'w'), indent=1)
print('structured/special', stats)
