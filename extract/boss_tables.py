"""ROM data tables used by the boss / multi-part object opcodes (game/src/game/objects/bosses.rs).

Writes extract/out/objects/tables.json: {"0x180BC": [ints...], ...}; `python3 extract/opcodes_game.py`
merges it into game/assets/objects/opcodes.ops.json ("tables"), read in Rust via `ctx.ops.tables`.
Every table is stored flat as signed ints; the layout of each one is documented below. All bounds were
checked against the disassembly: each table ends exactly where the next opcode routine starts.
"""
import json, os, struct

HERE = os.path.dirname(os.path.abspath(__file__))
ROM = open(os.path.join(HERE, '..', 'Cadash (USA, Korea) (En).md'), 'rb').read()


def s8(a):
    return struct.unpack('>b', ROM[a:a + 1])[0]


def s16(a):
    return struct.unpack('>h', ROM[a:a + 2])[0]


def u32(a):
    return struct.unpack('>I', ROM[a:a + 4])[0]


def s8_list(a, n):
    return [s8(a + i) for i in range(n)]


def s16_list(a, n):
    return [s16(a + 2 * i) for i in range(n)]


TABLES = {}

# --- s8 (dx, dy) pairs, read by 0x1A824 / 0x1A858 / 0x1A880 (entry i = values [2i, 2i+1]) ---------
# 0x180BC: Kraken arm tip path, 104 entries (B66).          0x1838C: arm sweep, 208 entries (B69).
# 0x18A40: spiked-ball chain tip, 60 entries (B74).          0x191B6: Worm segment orbit, 30 entries (B78).
# 0x177CE: grim reaper circle, 120 entries (B52).            0x1A4EA: Balrog tentacle tip, 72 entries (B126).
# 0x1971A: Fire Elemental ring, 40 entries; 0x1A880 doubles both values (B87).
for addr, n in [(0x180BC, 104), (0x1838C, 208), (0x18A40, 60), (0x191B6, 30), (0x177CE, 120),
                (0x1A4EA, 72), (0x1971A, 40)]:
    TABLES['0x%X' % addr] = s8_list(addr, 2 * n)

# --- s16 (dx, dy) pairs, read by 0x1A840 / 0x1A876 ---------------------------------------------
# 0x18C6A: sea-of-flame fire serpent tip, 120 entries (B75).  0x1949A: Fire Elemental cross radius, 55 (B84/B85).
for addr, n in [(0x18C6A, 120), (0x1949A, 55)]:
    TABLES['0x%X' % addr] = s16_list(addr, 2 * n)

# 0x17A34: B56 projectile volleys: 16 direction codes (0x1C2E2) x 4 s8 spawn-record indices (first 3 used).
TABLES['0x17A34'] = s8_list(0x17A34, 64)
# 0x17D04: B61 anim id per direction code (16 s8; -1 = keep the current anim).
TABLES['0x17D04'] = s8_list(0x17D04, 16)

# 0xCC4A: path-step turn limiter used by 0xCBE8 (B114 lightning): 9 rows (previous step 0..8) of 9 bytes
# (wanted step 0..8) -> step actually taken. The ROM stores row pointers (rows 1 and 4 share one row).
rows = []
for d in range(9):
    p = u32(0xCC4A + 4 * d)
    rows += [ROM[p + i] for i in range(9)]
TABLES['0xCC4A'] = rows
# 0x1C44E: path step velocity (integer px/frame words) for steps 0..8: [vx, vy] * 9
# (step = 3 * column + row: column 0 right / 1 centre / 2 left, row 0 up / 1 level / 2 down).
TABLES['0x1C44E'] = s16_list(0x1C44E, 18)
# 0x1C472: anim descriptor address per step 0..8 (sheet key "rom<ADDR>:0:<gfx>", see extract/objects_game.py).
TABLES['0x1C472'] = [u32(0x1C472 + 4 * d) for d in range(9)]

out = os.path.join(HERE, 'out', 'objects', 'tables.json')
os.makedirs(os.path.dirname(out), exist_ok=True)
json.dump(TABLES, open(out, 'w'))
print('wrote', out, {k: len(v) for k, v in TABLES.items()})
