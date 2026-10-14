"""Cadash LZSS decompressor (ROM routine at 0x2EE6)."""
import os
# Set by extract_assets.py (the verified cartridge image).
ROM_PATH = os.environ.get('CADASH_ROM', '')
def rom():
    return open(ROM_PATH, 'rb').read()

def decompress(data, src, clen, prefill=0x20):
    # output buffer; references before start read the 0xFEE-byte space-filled area semantics
    out = bytearray()
    i = src; end = src + clen
    PRE = 0x1000
    buf = bytearray([prefill]) * PRE  # virtual history before start
    while i < end:
        flags = data[i]; i += 1
        for bit in range(8):
            if i >= end: break
            if flags & 1:
                buf.append(data[i]); i += 1
            else:
                b1 = data[i]; b2 = data[i+1]; i += 2
                ln = (b2 & 0xF) + 3
                dist = (((b2 & 0xF0) << 4) | b1) + 1
                p = len(buf) - dist
                for _ in range(ln):
                    buf.append(buf[p]); p += 1
            flags >>= 1
    return bytes(buf[PRE:])
