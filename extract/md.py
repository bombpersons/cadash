import struct
# Colour levels per channel, matching the mdx renderer used for screenshot comparison.
LADDER=[0,32,65,98,139,172,205,238]
LADDER_G=[0,32,68,101,137,170,206,238]
def cram_rgb(word):
    r=(word>>1)&7; g=(word>>5)&7; b=(word>>9)&7
    return (LADDER[r],LADDER_G[g],LADDER[b])
def tile_pixels(data, idx):
    o=idx*32; t=data[o:o+32]
    if len(t)<32: t=t+bytes(32-len(t))
    px=[]
    for y in range(8):
        row=[]
        for x in range(4):
            b=t[y*4+x]; row+=[b>>4,b&15]
        px.append(row)
    return px
def be16(b,o): return struct.unpack('>H',b[o:o+2])[0]
def be32(b,o): return struct.unpack('>I',b[o:o+4])[0]
