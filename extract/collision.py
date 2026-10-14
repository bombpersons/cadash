"""Extract per-room collision (metatile attribute) data for Cadash.

For each room 0..47:
  attrs  : 512 attribute bytes (one per metatile index; table 0x63318 + room*6 -> {ptr,len}, LZSS -> RAM 0xFF6BA4)
  grid   : per 16x16 block attribute (attrs[map_cell & 0x1FF]), row-major, height rows of width ints
  kinds  : same grid but decoded to short kind strings (see LEGEND)
See docs/re/physics.md for the semantics (routines 0x3536..0x39DE, 0x34A0, 0x351A, 0x4238, 0xA508, 0xC486).
Usage: python3 extract/collision.py [outdir] [--ascii ROOM]
"""
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lzss import rom, decompress
from md import be16, be32

LEGEND = {
    "0x00": "empty (air)",
    "0x80": "solid: bit7 set -> blocks floor (falling), ceiling (rising), left/right walls. Any value with bit7 set is solid.",
    "0x01": "climbable rope/ladder: grabbed when the tile at (x, y - height/2) == 0x01 and player is airborne (routine 0x34A0)",
    "0x02": "hazard floor (spikes/fire): while tile at (x, y-1) == 0x02 and not invulnerable -> HP -= 2 and knockback (0x351A/0xA508)",
    "0x04": "no player/physics effect found (only room 25)",
    "0x06": "no player/physics effect found (rooms 2,4)",
    "0x1n": "walk-on surface / slope, top-only (one-way, checked only when vy >= 0). n = height-mask index: "
            "0 = 2:1 slope descending to the right, lower half (surface y 9..16); 1 = same slope upper half (surface y 1..8); "
            "2 = 2:1 slope rising to the right, lower half (surface y 16..9); 3 = rising, upper half (surface 8..1); "
            "4 = flat one-way platform (surface y 0); 5 = flat one-way platform (surface y 1); 6 = nothing",
    "0x2n": "door: press UP while standing in it (on ground, not attacking) -> room transition script 0x1388A, n = exit index",
    "0x3n": "in-room stair warp: press UP -> y += dy[n] from table 0x4992 (n even = up, odd = down), player frozen briefly",
    "0x4n": "auto exit: walking into it (on ground, not attacking) triggers room transition script 0x13572, n = exit index",
    "0x5n": "shop/house entrance: press UP -> script 0x13B82, n = index",
    "0x60": "room 12: entering switches player into 'walk-only' mode (+116 = 1: 1 px/f, no jump/attack)",
    "0x61": "room 12: entering switches player back to normal mode (+116 = 0)",
    "0x70": "no player/physics effect found (room 19)",
}

# height masks (ROM 0x36C4 -> 7 tables of 16 rows, bit x set = solid pixel at column x)
def slope_masks(r):
    out = []
    for i in range(7):
        p = be32(r, 0x36C4 + 4 * i)
        out.append([be16(r, p + 2 * y) for y in range(16)])
    return out

def kind(a):
    if a & 0x80: return "solid"
    if a == 0: return "empty"
    if a == 1: return "climb"
    if a == 2: return "hazard"
    hi, lo = a >> 4, a & 15
    if hi == 1: return "surface%d" % lo
    if hi == 2: return "door%d" % lo
    if hi == 3: return "warp%d" % lo
    if hi == 4: return "exit%d" % lo
    if hi == 5: return "shop%d" % lo
    if a == 0x60: return "mode_walk"
    if a == 0x61: return "mode_normal"
    return "unknown_%02x" % a

def room(r, n):
    p = be32(r, 0x63318 + n * 6); l = be16(r, 0x63318 + n * 6 + 4)
    attrs = list(decompress(r, p, l)[:512])
    mp = be32(r, 0x52BC0 + n * 12 + 6); ml = be16(r, 0x52BC0 + n * 12 + 10)
    m = decompress(r, mp, ml)
    w = m[0] * m[2]; h = m[1] * m[3]
    grid = [[attrs[be16(m, 4 + 2 * (y * w + x)) & 0x1FF] for x in range(w)] for y in range(h)]
    return dict(room=n, attr_src=hex(p), attr_len=l, width=w, height=h,
                screens=[m[0], m[1]], screen_blocks=[m[2], m[3]],
                max_scroll=[(m[0] - 1) * m[2] * 16, (m[1] - 1) * m[3] * 16],
                attrs=attrs, grid=grid)

ASCII = {"solid": "#", "empty": ".", "climb": "H", "hazard": "^"}
def ascii_row(row):
    s = ""
    for a in row:
        k = kind(a)
        s += ASCII.get(k) or {"surface": "_", "door": "D", "warp": "W", "exit": "E", "shop": "S", "mode_walk": "m",
                               "mode_normal": "n"}.get(k.rstrip("0123456789"), "?")
    return s

def main():
    args = sys.argv[1:]
    r = rom()
    if "--ascii" in args:
        d = room(r, int(args[args.index("--ascii") + 1]))
        for y, row in enumerate(d["grid"]): print("%3d %s" % (y, ascii_row(row)))
        return
    out = args[0] if args else os.path.join(os.path.dirname(os.path.abspath(__file__)), "out", "collision")
    os.makedirs(out, exist_ok=True)
    masks = slope_masks(r)
    for n in range(48):
        d = room(r, n)
        d["legend"] = LEGEND
        d["kinds"] = [[kind(a) for a in row] for row in d["grid"]]
        d["surface_masks"] = masks
        d["notes"] = ("grid[y][x] is the attribute of the 16x16 block at world pixel (x*16, y*16). "
                      "Game RAM coords = world + 0x80 on both axes (collision code subtracts 0x80). "
                      "surface_masks[n][row] bit c set = pixel (c,row) inside the block is ground for kind surface<n>.")
        with open(os.path.join(out, "room_%02d.json" % n), "w") as f:
            json.dump(d, f, separators=(",", ":"))
    print("wrote", out)

if __name__ == "__main__":
    main()
