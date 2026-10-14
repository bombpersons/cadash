"""Parse Cadash's SMPS (Z80) music data to find each song's intro and loop length.

The driver (Z80 code at ROM 0xC82) plays from the bank at ROM 0x38000 (Z80 0x8000).
Song table: header word +4 at 0x38000 -> list of song pointers (IDs 0x81..0x97).
Timing: every frame each track's duration counter decrements; every `tempo` frames all
counters are bumped by one (one frame skipped), so ticks advance at (tempo-1)/tempo per frame.
"""
import struct, sys, json

ROM = open('Cadash (USA, Korea) (En).md', 'rb').read()
BANK = 0x38000
def b(z): return ROM[BANK + z - 0x8000]
def w(z): return b(z) | b(z + 1) << 8

# parameter bytes for coordination flags E0..FF (excluding special handling)
PARAMS = {0xE0:1,0xE1:1,0xE2:1,0xE4:5,0xE5:2,0xE6:1,0xE7:0,0xE8:1,0xE9:2,0xEA:3,0xEC:1,0xED:2,0xEE:2,
          0xF0:4,0xF1:2,0xF3:1,0xF4:1,0xF5:1,0xFA:1,0xFB:1,0xFC:1,0xFD:1,0xFE:4}
META = {0:1,1:1,2:1,3:1,4:3,5:1,6:4,7:2,8:1,9:1}

def walk(ptr, mult, psg, max_ticks=200000):
    """Return (end_kind, events) where events maps position -> tick at first visit; on the final
    backwards F6 jump returns ('loop', target_tick, jump_tick)."""
    pc = ptr; tick = 0; dur = 0; stack = []; loops = {}; freq_mode = False; pan_mode = False
    seen = {}  # (pc, stack, loops) -> tick
    tempo_changes = []
    while tick < max_ticks:
        key = (pc, tuple(stack), tuple(sorted(loops.items())))
        if key in seen:
            return dict(kind='loop', start=seen[key], end=tick, tempo_changes=tempo_changes)
        seen[key] = tick
        c = b(pc); pc += 1
        if c >= 0xE0:
            if c in (0xF2, 0xE3): return dict(kind='stop', end=tick, tempo_changes=tempo_changes)
            if c == 0xF6: pc = w(pc); continue
            if c == 0xF7:
                idx, cnt = b(pc), b(pc + 1)
                k = idx
                if k not in loops: loops[k] = cnt
                loops[k] -= 1
                if loops[k] != 0: pc = w(pc + 2)
                else: del loops[k]; pc += 4
                continue
            if c == 0xF8: stack.append(pc + 2); pc = w(pc); continue
            if c == 0xF9: pc = stack.pop(); continue
            if c == 0xEF:
                v = b(pc); pc += 1 if v < 0x80 else 2; continue
            if c == 0xEB:
                hi = b(pc + 1); tempo_changes.append((tick, 'EB', [b(pc + i) for i in range(3 if (hi & 0x30) == 0x30 else 2)]))
                pc += 3 if (hi & 0x30) == 0x30 else 2; continue
            if c == 0xEA: tempo_changes.append((tick, 'EA', [b(pc), b(pc + 1), b(pc + 2)])); pc += 3; continue
            if c == 0xFC: pan_mode = b(pc) == 1; pc += 1; continue    # FC 01: notes carry an extra byte
            if c == 0xFD: freq_mode = b(pc) == 1; pc += 1; continue
            if c == 0xFF:
                m = b(pc); 
                if m in (0, 1, 8, 9): tempo_changes.append((tick, 'FF%02x' % m, b(pc + 1)))
                pc += 1 + META[m]; continue
            pc += PARAMS[c]; continue
        if freq_mode and c >= 0x80 or freq_mode:
            # 2-byte frequency (first byte already read), optional pan byte, then a duration
            pc += 1
            if pan_mode: pc += 1
            dur = b(pc) * mult; pc += 1; tick += dur; continue
        if c < 0x80:
            dur = c * mult; tick += dur; continue
        # note
        if pan_mode:
            pc += 1; dur = b(pc) * mult; pc += 1
        elif b(pc) < 0x80:
            dur = b(pc) * mult; pc += 1
        tick += dur
    return dict(kind='runaway', end=tick, tempo_changes=tempo_changes)

def song(sid):
    tbl = w(0x8000 + 4)
    p = w(tbl + 2 * (sid - 0x81))
    nfm, npsg, mult, tempo = b(p + 2), b(p + 3), b(p + 4), b(p + 5)
    tracks = []
    q = p + 6
    for i in range(nfm):
        tracks.append(('fm%d' % i, w(q))); q += 4
    for i in range(npsg):
        tracks.append(('psg%d' % i, w(q))); q += 6
    res = dict(id=sid, ptr=hex(p), fm=nfm, psg=npsg, mult=mult, tempo=tempo, tracks={})
    for name, tp in tracks:
        res['tracks'][name] = walk(tp, mult, name.startswith('psg'))
    return res

if __name__ == '__main__':
    for sid in range(0x81, 0x98):
        s = song(sid)
        tr = ' '.join('%s:%s %s-%s' % (k, v['kind'][0], v.get('start', ''), v['end']) for k, v in s['tracks'].items())
        print(hex(sid), 'mult', s['mult'], 'tempo', s['tempo'], tr)
        for k, v in s['tracks'].items():
            if v['tempo_changes']: print('   ', k, v['tempo_changes'][:6])


def priority(sid):
    """Priority byte of a sound (table at header word +0): a new sound only starts when its priority is
    at least that of the sound effect currently playing (Z80 0x08A7)."""
    return b(w(0x8000) + sid - 0x81)


def sfx_channels(sid):
    """Hardware channels a sound effect takes over (0x05/0x06 = FM 5/6, 0x80/0xA0/0xC0 = PSG 1-3)."""
    p = w(w(0x8000 + 6) + 2 * (sid - 0x98))
    return [b(p + 4 + 6 * k + 1) for k in range(b(p + 3))]
