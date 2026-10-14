import sys; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from md import *; from lzss import rom
r=rom()
def parse_desc(p):
    h=[be16(r,p+j) for j in range(0,8,2)]; last=h[2]
    fr=[(be16(r,p+8+i*10),be32(r,p+10+i*10),be32(r,p+14+i*10)) for i in range(last+1)]
    return h,fr,p+8+(last+1)*10
def parse_block(p,end):
    out=[]
    while p<end:
        a=be32(r,p); b=be32(r,p+4)
        if a!=p+8: break
        hr,fr,q=parse_desc(a)
        if q!=b: break
        hl,fl,q2=parse_desc(b)
        out.append((p,hr,fr,hl,fl)); p=q2
    return out,p
if __name__=='__main__':
    for s in (int(x,16) for x in sys.argv[1:]):
        # search backward for block start
        out,end=parse_block(s,s+0x2000)
        print(hex(s),'anims',len(out),'end',hex(end))
        for p,hr,fr,hl,fl in out:
            print(' ',hex(p),hr,[(d,hex(t),hex(m)) for d,t,m in fr])
