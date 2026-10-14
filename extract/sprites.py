import sys; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from md import *; from lzss import rom, decompress
from PIL import Image
r=rom()
def mapping_pieces(mp):
    n=be16(r,mp); hdr=[struct.unpack('>h',r[mp+2+i*2:mp+4+i*2])[0] for i in range(8)]
    pcs=[]
    for k in range(n):
        q=mp+18+k*8
        if be16(r,q)==0x8000: continue
        dx=struct.unpack('>h',r[q:q+2])[0]; dy=struct.unpack('>h',r[q+2:q+4])[0]
        size=be16(r,q+4)&0xFF; attr=be16(r,q+6)
        pcs.append((dx,dy,size,attr))
    return hdr,pcs
def stage_tiles(tl, bank):
    n=be16(r,tl); out=bytearray(); q=tl+2
    for k in range(n):
        ln=be16(r,q); idx=be16(r,q+2); q+=4
        out+=bank[idx*32: idx*32+ln]
    return bytes(out)
def render_indexed(pieces, vram, base_attr=0):
    """Like render(), but each pixel holds palette line*16 + colour (0 = transparent) in an 'L' image."""
    if not pieces: return None,0,0
    boxes=[]
    for dx,dy,size,attr in pieces:
        w=((size>>2)&3)+1; h=(size&3)+1
        boxes.append((dx,dy,dx+w*8,dy+h*8))
    x0=min(b[0] for b in boxes); y0=min(b[1] for b in boxes); x1=max(b[2] for b in boxes); y1=max(b[3] for b in boxes)
    img=Image.new('L',(x1-x0,y1-y0),0); px=img.load()
    for dx,dy,size,attr in reversed(pieces):
        attr|=base_attr
        w=((size>>2)&3)+1; h=(size&3)+1
        t0=attr&0x7FF; p=(attr>>13)&3; hf=attr&0x800; vf=attr&0x1000
        for cx in range(w):
            for cy in range(h):
                tile=tile_pixels(vram,t0+cx*h+cy)
                tx=(w-1-cx) if hf else cx; ty=(h-1-cy) if vf else cy
                for y in range(8):
                    for x in range(8):
                        c=tile[7-y if vf else y][7-x if hf else x]
                        if c: px[dx-x0+tx*8+x, dy-y0+ty*8+y]=p*16+c
    return img,-x0,-y0

def render(pieces, vram, pal, base_attr=0):
    """returns (Image RGBA, origin_x, origin_y) origin = offset of object position inside image"""
    if not pieces: return None,0,0
    boxes=[]
    for dx,dy,size,attr in pieces:
        w=((size>>2)&3)+1; h=(size&3)+1
        boxes.append((dx,dy,dx+w*8,dy+h*8))
    x0=min(b[0] for b in boxes); y0=min(b[1] for b in boxes); x1=max(b[2] for b in boxes); y1=max(b[3] for b in boxes)
    img=Image.new('RGBA',(x1-x0,y1-y0),(0,0,0,0)); px=img.load()
    # draw in reverse order so first piece is on top (VDP: lower index = higher priority)
    for dx,dy,size,attr in reversed(pieces):
        attr|=base_attr
        w=((size>>2)&3)+1; h=(size&3)+1
        t0=attr&0x7FF; p=(attr>>13)&3; hf=attr&0x800; vf=attr&0x1000
        for cx in range(w):
            for cy in range(h):
                tile=tile_pixels(vram,t0+cx*h+cy)
                tx=(w-1-cx) if hf else cx; ty=(h-1-cy) if vf else cy
                for y in range(8):
                    for x in range(8):
                        c=tile[7-y if vf else y][7-x if hf else x]
                        if c: px[dx-x0+tx*8+x, dy-y0+ty*8+y]=pal[p*16+c]+(255,)
    return img,-x0,-y0
