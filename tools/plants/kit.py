"""A small toolkit for drawing 32x32 plant sprites as palette-indexed text grids.

Materials: 'L' leaf (shades 0..5), 'P' petal (0..4), 'C' centre (0..2), 'S' second petal (0..4).
Light comes from the top left: surfaces facing it take lighter shades, and every shape gets
a one-shade darker rim on its lower right, so sprites read cleanly at a glance."""
import math
N=32
CH={'L':'abcdef','P':'pqrst','C':'xyz','S':'jklmn','T':'uvw','F':'ghio','W':'ABDE','G':'GHI'}
class Canvas:
    def __init__(s,n=N): s.n=n; s.g=[[None]*n for _ in range(n)]
    def put(s,x,y,mat,sh):
        x,y=int(round(x)),int(round(y))
        if 0<=x<s.n and 0<=y<s.n:
            sh=max(0,min(len(CH[mat])-1,int(sh))); s.g[y][x]=(mat,sh)
    def get(s,x,y):
        return s.g[y][x] if 0<=x<s.n and 0<=y<s.n else None
    def rows(s):
        return [''.join('.' if c is None else CH[c[0]][c[1]] for c in r) for r in s.g]
# --- primitives -------------------------------------------------------------
def line(c,x0,y0,x1,y1,mat,sh,width=1):
    steps=max(abs(x1-x0),abs(y1-y0),1)*2
    for i in range(int(steps)+1):
        t=i/steps; x=x0+(x1-x0)*t; y=y0+(y1-y0)*t
        for w in range(width): c.put(x+w,y,mat,sh)
def stem(c,pts,sh=2,width=1):
    """A stem through points (bottom first), dark on its right side."""
    for (x0,y0),(x1,y1) in zip(pts,pts[1:]):
        line(c,x0,y0,x1,y1,'L',sh,width)
    if width==1:
        for (x0,y0),(x1,y1) in zip(pts,pts[1:]):
            line(c,x0+1,y0,x1+1,y1,'L',sh-1) if False else None
def curve(x0,y0,x1,y1,bend,k=6):
    """Points along a gentle quadratic curve from (x0,y0) to (x1,y1), bowed sideways by bend."""
    mx,my=(x0+x1)/2+bend,(y0+y1)/2
    return [((1-t)**2*x0+2*(1-t)*t*mx+t*t*x1,(1-t)**2*y0+2*(1-t)*t*my+t*t*y1) for t in [i/k for i in range(k+1)]]
def leaf(c,x,y,ang,length,width,base_sh=3,mat='L'):
    """A pointed leaf from (x,y) along angle (degrees, 0 = right, -90 = up), with a midrib."""
    a=math.radians(ang); dx,dy=math.cos(a),math.sin(a); px,py=-dy,dx
    for i in range(int(length*4)+1):
        t=i/(length*4); half=width*math.sin(math.pi*t)**0.8/2
        for j in range(-int(half*4),int(half*4)+1):
            o=j/4; X=x+dx*t*length+px*o; Y=y+dy*t*length+py*o
            # side facing up-left lighter
            side = 1 if (px*o*-1+py*o*-1)>0 else 0
            sh=base_sh+ (1 if side and abs(o)>0.6 else 0) - (1 if not side and abs(o)>half-0.8 else 0)
            c.put(X,Y,mat,sh)
    for i in range(int(length*2)):
        t=i/(length*2); c.put(x+dx*t*length*0.85,y+dy*t*length*0.85,mat,base_sh-1)
def disc(c,cx,cy,r,mat,sh,rim=True,shade=True):
    for y in range(int(cy-r-1),int(cy+r+2)):
        for x in range(int(cx-r-1),int(cx+r+2)):
            d=math.hypot(x-cx,y-cy)
            if d<=r+0.25:
                s=sh
                if shade:
                    lx=(x-cx)/max(r,0.5); ly=(y-cy)/max(r,0.5)
                    if lx+ly<-0.6: s+=1
                    if lx+ly>0.7: s-=1
                if rim and d>r-0.75 and (x-cx)+(y-cy)>0: s-=1
                c.put(x,y,mat,s)
def petal(c,cx,cy,ang,length,width,mat='P',sh=3,tip_round=True):
    """A petal from the flower centre outwards, lit on its upper-left side."""
    a=math.radians(ang); dx,dy=math.cos(a),math.sin(a); px,py=-dy,dx
    n=int(length*4)
    for i in range(n+1):
        t=i/n
        prof=math.sin(math.pi*min(1,t*0.9+0.1))**0.6 if tip_round else math.sin(math.pi*t)
        half=width*prof/2
        for j in range(-int(half*4),int(half*4)+1):
            o=j/4; X=cx+dx*t*length+px*o; Y=cy+dy*t*length+py*o
            facing=-(px*o+py*o)
            s=sh+(1 if facing>0.5 else 0)-(1 if facing<-0.5 else 0)-(1 if t<0.25 else 0)
            c.put(X,Y,mat,s)
def rim(c,mat=None):
    """Darken the lower-right edge of every shape by one shade, against what is behind it."""
    g=[row[:] for row in c.g]
    for y in range(c.n):
        for x in range(c.n):
            v=g[y][x]
            if v is None or (mat and v[0]!=mat): continue
            for dx,dy in ((1,0),(0,1)):
                nx,ny=x+dx,y+dy
                if not(0<=nx<c.n and 0<=ny<c.n) or g[ny][nx] is None:
                    c.g[y][x]=(v[0],max(0,v[1]-1)); break
def tuft(c,x,y,n,h,spread=4,sh=3,seed=1):
    """Grass blades at the foot of a plant."""
    import random; r=random.Random(seed)
    for i in range(n):
        bx=x+r.uniform(-spread,spread); top=h*r.uniform(0.5,1.0); lean=r.uniform(-2.5,2.5)
        pts=curve(bx,y,bx+lean,y-top,lean*0.3,4)
        for k,((x0,y0),(x1,y1)) in enumerate(zip(pts,pts[1:])):
            line(c,x0,y0,x1,y1,'L',sh+(1 if k>2 else 0)-(1 if k==0 else 0))

def blade_leaf(c,x,y,ang,length,width,sh=3,mat='L',droop=0.0):
    """A lanceolate leaf that may droop at its tip: lit half above the midrib, shaded below."""
    a0=math.radians(ang)
    pts=[]
    n=int(length*3)+2
    for i in range(n+1):
        t=i/n; a=a0+droop*t*t
        if i==0: px,py=x,y
        else:
            px+=math.cos(a)*length/n; py+=math.sin(a)*length/n
        pts.append((px,py,a,t))
    for px,py,a,t in pts:
        half=width*(math.sin(math.pi*min(1,t*1.05))**0.7)/2
        nx,ny=-math.sin(a),math.cos(a)
        k=int(half*4)
        for j in range(-k,k+1):
            o=j/4; X=px+nx*o; Y=py+ny*o
            up = (nx*o+ny*o) < 0   # toward the top-left
            s = sh+1 if up else sh-1
            if abs(o)<0.35: s=sh
            c.put(X,Y,mat,s)
def cup(c,cx,cy,w,h,mat='P',sh=2,lobes=3):
    """A tulip-like cup: rounded bottom, lobed top, lit from the left."""
    for y in range(int(cy-h),int(cy)+1):
        t=(cy-y)/h   # 0 bottom .. 1 top
        half=w/2*math.sqrt(max(0,1-(1-t)**2*0.9)) if t<0.999 else w/2
        for x in range(int(cx-half),int(cx+half)+1):
            u=(x-cx)/max(half,0.5)
            s=sh+(1 if u<-0.3 else 0)+(1 if u<-0.7 else 0)-(1 if u>0.55 else 0)
            # lobes: notch the top edge
            if t>0.8:
                ph=(u+1)/2*lobes
                if abs(ph-round(ph))<0.18 and abs(u)<0.9: continue
            c.put(x,y,mat,s)
def bell(c,cx,cy,r,mat='P',sh=2):
    """A hanging bell, mouth down, with flared tips (r is about 2 for a small flower)."""
    cx=int(round(cx)); cy=int(round(cy))
    rows=[(-1,0),(-1,1),(-2,1),(-2,2)] if r>=1.8 else [(0,0),(-1,1),(-1,1)]
    for k,(a,b) in enumerate(rows):
        for x in range(cx+a,cx+b+1):
            s2=sh+(1 if x<cx else 0)-(1 if x>cx else 0)
            c.put(x,cy-len(rows)+1+k,mat,s2)
    y=cy+1
    c.put(cx+rows[-1][0]-1,y,mat,sh+1); c.put(cx+rows[-1][1]+1,y,mat,sh-1)
    c.put(cx,y,mat,sh-1)
def floret_spike(c,x,y0,y1,w,mat='P',sh=2,seed=0):
    """Small florets stacked along a spike (lavender, lupine, foxglove), tapering upward."""
    r=random.Random(seed); n=int((y0-y1))
    for i in range(n+1):
        y=y0-i; t=i/max(n,1); half=w*(1-t*0.7)/2
        for side in (-1,1):
            if r.random()<0.85:
                c.put(x+side*half,y,'P',sh+(1 if side<0 else -0))
                if half>1: c.put(x+side*(half-1),y,'P',sh+1 if side<0 else sh)
        c.put(x,y,'P',sh+1 if i%2 else sh)
import random


def ellipse(c,cx,cy,rx,ry,mat,sh,jag=0.0,seed=0,keep=1.0):
    """A shaded ellipse (bush, cap, fruit), lit from the top left; `jag` roughens its edge."""
    r=random.Random(seed)
    for y in range(int(cy-ry-2),int(cy+ry+3)):
        for x in range(int(cx-rx-2),int(cx+rx+3)):
            u,v=(x-cx)/rx,(y-cy)/ry; d=math.hypot(u,v)
            if d<=1+jag*(r.random()-0.5) and r.random()<keep:
                lit=-(u+v)
                s=sh+(1 if lit>0.5 else 0)+(1 if lit>1.0 else 0)-(1 if lit<-0.6 else 0)-(1 if d>0.85 and u+v>0 else 0)
                c.put(x,y,mat,s)
def foliage(c,cx,cy,rx,ry,sh=3,seed=0,leaf_len=2.6,density=1.0):
    """A bush crown of many small leaves, lit from the top left, darker inside."""
    r=random.Random(seed)
    ellipse(c,cx,cy,rx*0.9,ry*0.9,'L',sh-1,jag=0.3,seed=seed)
    n=int(rx*ry*0.55*density)
    for i in range(n):
        a=r.uniform(0,2*math.pi); d=math.sqrt(r.random())
        x=cx+math.cos(a)*d*rx; y=cy+math.sin(a)*d*ry
        lit=-(math.cos(a)+math.sin(a))*d
        leaf(c,x,y,r.uniform(0,360),leaf_len*r.uniform(0.7,1.2),leaf_len*0.8,sh+(1 if lit>0.3 else 0)-(1 if lit<-0.5 else 0))
def berry(c,x,y,mat='F',size=1):
    """A round berry with a highlight."""
    x,y=int(round(x)),int(round(y))
    if size<=1:
        c.put(x,y,mat,2); c.put(x+1,y,mat,1); c.put(x,y+1,mat,1); c.put(x+1,y+1,mat,0)
        c.put(x,y,mat,3)
    else:
        ellipse(c,x,y,size,size,mat,1); c.put(x-size//2,y-size//2,mat,3)
def berries(c,cx,cy,rx,ry,n,seed=0,mat='F',size=1,cluster=1):
    r=random.Random(seed+99)
    for i in range(n):
        a=r.uniform(0,2*math.pi); d=math.sqrt(r.random())*0.85
        x=cx+math.cos(a)*d*rx; y=cy+math.sin(a)*d*ry
        for k in range(cluster):
            berry(c,x+(k%2)*2-(k//2),y+(k//2)*2,mat,size)
def twigs(c,x,y,n,h,spread,sh=1,seed=0):
    r=random.Random(seed)
    for i in range(n):
        a=-90+r.uniform(-spread,spread); l=h*r.uniform(0.6,1.0)
        x1=x+math.cos(math.radians(a))*l; y1=y+math.sin(math.radians(a))*l
        line(c,x,y,x1,y1,'W',sh+(1 if i%2 else 0))
        if l>5:
            mx,my=(x+x1)/2,(y+y1)/2; a2=a+r.choice((-35,35))
            line(c,mx,my,mx+math.cos(math.radians(a2))*l*0.4,my+math.sin(math.radians(a2))*l*0.4,'W',sh)
def feather_leaf(c,x,y,ang,length,sh=3,mat='L'):
    """A fine, divided leaf (carrot, dill, fern frond): a rib with short side strokes."""
    a=math.radians(ang); dx,dy=math.cos(a),math.sin(a)
    n=int(length)
    for i in range(n):
        px,py=x+dx*i,y+dy*i; c.put(px,py,mat,sh-1)
        if i>0 and i%2==0:
            w=max(1,int((1-i/n)*3))
            for side in (-1,1):
                for k in range(1,w+1):
                    c.put(px-dy*side*k+dx*k*0.5,py+dx*side*k+dy*k*0.5,mat,sh+(1 if side<0 else 0))
