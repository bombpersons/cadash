"""Tiny VDP-free compositor helpers for the non-gameplay screens (ROM -> PNG layers).
A 'vram' here is just a bytearray(0x10000) we fill from ROM data the same way the game's loaders do."""
import struct, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lzss import decompress, rom as _rom
from md import cram_rgb
from PIL import Image

ROM = _rom()
def be16(b, o): return struct.unpack('>H', b[o:o+2])[0]
def s16(b, o): return struct.unpack('>h', b[o:o+2])[0]
def be32(b, o): return struct.unpack('>I', b[o:o+4])[0]
def words(b, o, n): return [be16(b, o+2*i) for i in range(n)]

def lz(src, clen): return decompress(ROM, src, clen)

class VRAM:
    def __init__(self): self.m = bytearray(0x10000)
    def put(self, addr, data): self.m[addr:addr+len(data)] = data
    def put_nt(self, addr, cells, w, h, pitch=64):
        """write w*h cell words (row-major) at nametable address (rect copy like 0x2DB6)"""
        for y in range(h):
            for x in range(w):
                a = addr + (y*pitch + x)*2
                self.m[a:a+2] = struct.pack('>H', cells[y*w+x])
    def fill_nt(self, addr, w, h, val, pitch=64):
        self.put_nt(addr, [val]*(w*h), w, h, pitch)
    def put_text(self, addr, s, base, pitch=64):
        """like 0x2C40: one byte per cell, cell = byte + base"""
        for i, c in enumerate(s):
            a = addr + i*2
            self.m[a:a+2] = struct.pack('>H', (c + base) & 0xFFFF)
    def cell(self, base, x, y, pitch=64): return be16(self.m, base + (y*pitch + x)*2)

def pal_rgb(cram_words):
    return [cram_rgb(w) for w in cram_words]

def draw_tile(img, vram, tile, x0, y0, pal, hf=False, vf=False, pr_filter=None):
    o = (tile & 0x7FF)*32
    px = img.load()
    W, H = img.size
    for y in range(8):
        for x in range(8):
            b = vram[o + y*4 + x//2]
            c = (b >> 4) if x % 2 == 0 else (b & 15)
            if c == 0: continue
            xx = x0 + (7-x if hf else x); yy = y0 + (7-y if vf else y)
            if 0 <= xx < W and 0 <= yy < H:
                r, g, bb = pal[c]
                px[xx, yy] = (r, g, bb, 255)

def render_plane(vm, base, cram, w=64, h=32, scroll_x=0, scroll_y=0, out_w=320, out_h=224, priority=None):
    """Render a 64x32 plane as RGBA (colour 0 transparent). priority: None=all, True/False=only that priority."""
    pal = pal_rgb(cram)
    img = Image.new('RGBA', (out_w, out_h), (0, 0, 0, 0))
    for ty in range(-1, out_h//8 + 2):
        for tx in range(-1, out_w//8 + 2):
            cx = (tx + scroll_x//8) % w; cy = (ty + scroll_y//8) % h
            v = vm.cell(base, cx, cy, w)
            if priority is not None and bool(v & 0x8000) != priority: continue
            p = (v >> 13) & 3
            draw_tile(img, vm.m, v & 0x7FF, tx*8 - scroll_x % 8, ty*8 - scroll_y % 8, pal[p*16:p*16+16],
                      bool(v & 0x800), bool(v & 0x1000))
    return img

def render_rect(vm, base, cram, cx0, cy0, cw, ch, pitch=64):
    """Render a sub-rectangle of a plane (cells) to its own RGBA image."""
    pal = pal_rgb(cram)
    img = Image.new('RGBA', (cw*8, ch*8), (0, 0, 0, 0))
    for y in range(ch):
        for x in range(cw):
            v = vm.cell(base, cx0+x, cy0+y, pitch)
            p = (v >> 13) & 3
            draw_tile(img, vm.m, v & 0x7FF, x*8, y*8, pal[p*16:p*16+16], bool(v & 0x800), bool(v & 0x1000))
    return img

def mapping_pieces(a, header=False):
    """Sprite mapping at ROM a: w count, [16-byte header], count x {dx,dy,size,attr}"""
    n = be16(ROM, a); o = a + 2 + (16 if header else 0); out = []
    for i in range(n):
        p = o + i*8
        if be16(ROM, p) == 0x8000: continue
        out.append(dict(dx=s16(ROM, p), dy=s16(ROM, p+2), size=be16(ROM, p+4) & 0xFF, attr=be16(ROM, p+6)))
    return out

def render_pieces(vm, pieces, cram, ox, oy, img=None, attr_or=0):
    """Draw VDP-style sprite pieces at origin (ox,oy) in screen px (already minus 0x80)."""
    pal = pal_rgb(cram)
    if img is None: img = Image.new('RGBA', (320, 224), (0, 0, 0, 0))
    for pc in reversed(pieces):  # first piece = highest priority -> draw last
        s = pc['size']; w = ((s >> 2) & 3) + 1; h = (s & 3) + 1
        a = pc['attr'] | attr_or; t = a & 0x7FF; p = (a >> 13) & 3; hf = bool(a & 0x800); vf = bool(a & 0x1000)
        for cx in range(w):
            for cy in range(h):
                tile = t + cx*h + cy
                dx = (w-1-cx) if hf else cx; dy = (h-1-cy) if vf else cy
                draw_tile(img, vm.m, tile, ox + pc['dx'] + dx*8, oy + pc['dy'] + dy*8, pal[p*16:p*16+16], hf, vf)
    return img

def bbox_crop(img):
    bb = img.getbbox()
    return (img.crop(bb), bb) if bb else (img, (0, 0, 0, 0))

def font_vram(vm, dest=0x5000, first_tile=0xD0, ntiles=48):
    """font/UI bank LZSS 0x639C4 -> VRAM (game: tiles 0xD0..0xFF to 0x5000 = tile 0x280)"""
    bank = lz(0x639C4, 0xF8C)
    vm.put(dest, bank[first_tile*32:(first_tile+ntiles)*32])
    return bank
