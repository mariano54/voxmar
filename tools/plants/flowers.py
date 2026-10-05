import sys,math,random; sys.path.insert(0,__import__('os').path.dirname(__file__)); from kit import *
F={}
def reg(name,**pal):
    def d(fn): F[name]=(fn,pal); return fn
    return d
def leaves_base(c,x,y,n=4,length=8,width=2.4,seed=0,sh=3,droop=0.5):
    r=random.Random(seed)
    for i in range(n):
        side=-1 if i%2==0 else 1
        ang=-90+side*r.uniform(35,70)
        blade_leaf(c,x,y,ang,length*r.uniform(0.75,1.1),width,sh,droop=droop*side)
def stem_to(c,x0,x1,y1,bend=0,sh=2):
    stem(c,curve(x0,31,x1,y1,bend),sh)
def radial(c,cx,cy,n,length,width,sh=2,rot=0,alt=True):
    for i in range(n):
        a=rot+i*360/n
        l=length*(0.88 if alt and i%2 else 1.0)
        s=sh+(1 if math.sin(math.radians(a))<-0.2 else 0)-(1 if math.sin(math.radians(a))>0.6 else 0)
        petal(c,cx,cy,a,l,width,'P',s)

@reg('dandelion',leaf='hi_leaf',petal='hi_yellow',centre='hi_gold_centre',tuft='hi_seed')
def dandelion():
    c=Canvas(); leaves_base(c,16,30,6,9,2.6,seed=5,droop=0.6)
    stem_to(c,16,15,11,2,3); stem_to(c,16,23,15,1,2)
    radial(c,15,9,20,4.6,1.6,2); disc(c,15,9,2.4,'P',3)
    disc(c,23,12,2.8,'T',1)
    for i in range(12):
        a=math.radians(i*30); c.put(23+math.cos(a)*3.6,12+math.sin(a)*3.6,'T',2)
    rim(c); return c

@reg('poppy',leaf='hi_leaf',petal='hi_red',centre='hi_poppy_centre')
def poppy():
    c=Canvas(); leaves_base(c,15,30,5,8,2.6,seed=2,droop=0.7)
    stem_to(c,15,17,10,-3); stem_to(c,15,9,18,1)
    cx,cy=17,8
    for a,l,s in [(-125,5.6,1),(-55,5.6,2),(170,5.2,2),(10,5.2,1)]: petal(c,cx,cy,a,l,7,'P',s)
    for a,l,s in [(-90,3.2,3),(150,3.8,3),(30,3.8,2)]: petal(c,cx,cy+1,a,l,5,'P',s)
    disc(c,cx,cy,1.5,'C',1,shade=False); c.put(cx,cy,'C',2)
    for a in range(0,360,45): c.put(cx+math.cos(math.radians(a))*2.3,cy+math.sin(math.radians(a))*2.3,'C',0)
    disc(c,9,17,1.8,'L',3); c.put(8,16,'L',4); c.put(9,19,'P',2)
    rim(c); return c

@reg('cornflower',leaf='hi_leaf',petal='hi_blue',centre='hi_navy_centre')
def cornflower():
    c=Canvas()
    for (x1,y1,b) in [(15,9,-2),(22,14,2),(10,16,-1)]: stem_to(c,16,x1,y1,b)
    for (x,y,a) in [(15,25,-150),(16,23,-30),(15,20,-145),(16,28,-35)]: blade_leaf(c,x,y,a,6,1.6,3,droop=0.2)
    def head(cx,cy,s=1.0):
        for i in range(11):
            a=-185+i*19; petal(c,cx,cy,a,4.4*s,2.1*s,'P',2+(i%2))
            c.put(cx+math.cos(math.radians(a))*4.9*s,cy+math.sin(math.radians(a))*4.9*s,'P',3)
        disc(c,cx,cy+0.4,1.3*s,'C',1,shade=False)
        disc(c,cx,cy+2.3*s,1.5*s,'L',2)
    head(15,8); head(22,13,0.75); head(10,15,0.7)
    rim(c); return c

@reg('oxeye_daisy',leaf='hi_leaf',petal='hi_white',centre='hi_gold_centre')
def oxeye():
    c=Canvas(); leaves_base(c,16,30,5,7,2.4,seed=9)
    stem_to(c,16,14,10,-2); stem_to(c,16,23,15,2)
    for (x,y,a) in [(15,22,-150),(16,18,-35)]: blade_leaf(c,x,y,a,4.5,1.8,3)
    def head(cx,cy,s=1.0):
        radial(c,cx,cy,16,5.4*s,1.9*s,2,rot=6); disc(c,cx,cy,2.2*s,'C',1)
    head(14,9); head(23,14,0.7)
    rim(c); return c

@reg('azure_bluet',leaf='hi_leaf',petal='hi_pale_blue',centre='hi_gold_centre')
def bluet():
    c=Canvas(); leaves_base(c,16,30,6,5,2.0,seed=3,droop=0.3)
    heads=[(10,14,-2),(16,9,0),(22,13,2),(13,19,-1),(20,18,1)]
    for x,y,b in heads: stem_to(c,16,x,y+1,b)
    for x,y,b in heads:
        for i in range(4): petal(c,x,y,45+i*90,2.3,2.2,'P',3 if i in (2,3) else 2)
        c.put(x,y,'C',2)
    rim(c); return c

@reg('allium',leaf='hi_leaf',petal='hi_purple')
def allium():
    c=Canvas()
    for x,a in [(13,-110),(19,-70),(15,-100),(17,-80)]: blade_leaf(c,x,31,a,10,1.8,3,droop=0.6 if a>-90 else -0.6)
    stem_to(c,16,16,11,1,3)
    cx,cy,r=16,7,5.2
    rr=random.Random(4)
    for y in range(int(cy-r),int(cy+r)+1):
        for x in range(int(cx-r),int(cx+r)+1):
            d=math.hypot(x-cx,y-cy)
            if d<=r and rr.random()<0.92:
                lit=-(x-cx)-(y-cy)
                s=2+(1 if lit>2 else 0)+(1 if lit>5 else 0)-(1 if lit<-3 else 0)
                if rr.random()<0.25: s+=1
                c.put(x,y,'P',s)
    for i in range(10):
        a=rr.uniform(0,6.28); c.put(cx+math.cos(a)*(r+1),cy+math.sin(a)*(r+1),'P',2)
    rim(c); return c

@reg('blue_orchid',leaf='hi_leaf',petal='hi_cyan',centre='hi_orchid_centre')
def orchid():
    c=Canvas()
    for x,a in [(14,-140),(18,-40),(15,-160),(17,-20)]: blade_leaf(c,x,30,a,7,3.2,3,droop=0.3)
    stem(c,curve(16,30,14,6,4,8),2)
    for (x,y,s) in [(13,7,1.0),(18,12,0.9),(13,17,0.85)]:
        petal(c,x,y,-120,3.6*s,3.0*s,'P',2); petal(c,x,y,-60,3.6*s,3.0*s,'P',3)
        petal(c,x,y,160,3.0*s,2.4*s,'P',2); petal(c,x,y,20,3.0*s,2.4*s,'P',3)
        petal(c,x,y,90,3.4*s,2.6*s,'P',1); c.put(x,y,'C',2); c.put(x,y+1,'C',1)
    rim(c); return c

@reg('lily_of_the_valley',leaf='hi_leaf',petal='hi_white')
def lily():
    c=Canvas()
    blade_leaf(c,15,31,-100,13,5,3,droop=-0.4); blade_leaf(c,17,31,-75,12,5,2,droop=0.5)
    stem(c,curve(16,31,13,6,-6,8),2)
    for i,(x,y) in enumerate([(10,8),(15,9),(11,13),(17,13),(13,17)]):
        line(c,x+1,y-3,x,y-2,'L',2)
        bell(c,x,y,1.6,'P',2); c.put(x-1,y-1,'P',4)
    rim(c); return c

def tulip():
    c=Canvas()
    blade_leaf(c,15,31,-110,13,4.2,3,droop=-0.5); blade_leaf(c,17,31,-70,11,4,2,droop=0.6)
    stem_to(c,16,16,13,1,3)
    cup(c,16,13,8,8,'P',2,lobes=3)
    for y in range(6,12): c.put(15,y,'P',3)
    rim(c); return c
for col in ('red','orange','white','pink'):
    F[f'{col}_tulip']=(tulip,dict(leaf='hi_leaf_blue',petal=f'hi_{col}'))

@reg('buttercup',leaf='hi_leaf',petal='hi_butter',centre='hi_gold_centre')
def buttercup():
    c=Canvas(); leaves_base(c,16,30,5,6,3.0,seed=11)
    heads=[(11,11,-2,1.0),(20,8,2,1.0),(16,16,0,0.8)]
    for x,y,b,s in heads: stem_to(c,16,x,y+2,b)
    for x,y,b,s in heads:
        for i in range(5): petal(c,x,y,-90+i*72,3.0*s,3.0*s,'P',2+(1 if i in (0,4) else 0))
        disc(c,x,y,1.0,'C',1,shade=False); c.put(x-1,y-1,'P',4)
    rim(c); return c

@reg('bluebell',leaf='hi_leaf',petal='hi_bluebell')
def bluebell():
    c=Canvas()
    for x,a in [(13,-120),(19,-60),(16,-95)]: blade_leaf(c,x,31,a,11,2.6,3,droop=0.6 if a>-90 else -0.6)
    stem(c,[(16,31),(17,24),(19,17),(20,11),(19,7),(17,5)],2)
    for (sx,sy,bx,by) in [(17,5,13,8),(19,9,15,14),(20,13,23,18),(19,17,15,21)]:
        line(c,sx,sy,bx+1,by-3,'L',2); bell(c,bx,by,2,'P',2)
    rim(c); return c

@reg('lavender',leaf='hi_sage_leaf',petal='hi_lavender')
def lavender():
    c=Canvas()
    leaves_base(c,16,30,8,7,1.4,seed=7,droop=0.3,sh=2)
    for i,(x1,y1,b) in enumerate([(9,6,-3),(16,3,0),(23,5,3),(12,9,-1),(20,9,1)]):
        stem_to(c,16,x1,y1+8,b,sh=2)
        floret_spike(c,x1,y1+8,y1,2.6,'P',2,seed=i)
    rim(c); return c

# --- wild flowers ---
def daisy_like(name, petals, plen, pw, centre_r, heads, leafpal='hi_leaf', **pal):
    def f():
        c=Canvas(); leaves_base(c,16,30,5,7,2.4,seed=len(name))
        for x,y,s,b in heads: stem_to(c,16,x,y+2,b)
        for x,y,s,b in heads:
            radial(c,x,y,petals,plen*s,pw*s,2,rot=len(name)*7)
            disc(c,x,y,centre_r*s,'C',1)
        rim(c); return c
    F[name]=(f,dict(leaf=leafpal,**pal))
daisy_like('chamomile',12,3.4,1.5,1.6,[(11,11,0.9,-2),(19,8,1.0,2),(15,16,0.75,0),(23,15,0.7,3)],petal='hi_white',centre='hi_gold_centre')
daisy_like('aster',18,4.4,1.3,1.8,[(12,10,1.0,-2),(21,12,0.9,2),(15,17,0.7,0)],petal='hi_aster',centre='hi_gold_centre')
daisy_like('marigold',22,4.2,2.2,1.4,[(15,9,1.0,0),(22,15,0.75,2)],petal='hi_marigold',centre='hi_rust_centre')
daisy_like('cosmos',8,5.2,2.8,1.4,[(13,9,1.0,-2),(22,14,0.8,2)],petal='hi_cosmos',centre='hi_gold_centre')
daisy_like('zinnia',16,4.0,2.4,1.6,[(15,9,1.0,0),(21,16,0.7,2)],petal='hi_zinnia',centre='hi_gold_centre')
daisy_like('st_johns_wort',5,3.0,2.6,1.0,[(11,10,1.0,-2),(20,9,1.0,2),(15,15,0.85,0),(22,17,0.7,3)],petal='hi_butter',centre='hi_gold_centre')
daisy_like('primrose',5,3.4,3.2,1.0,[(10,15,1.0,-2),(17,12,1.0,0),(22,17,0.9,2)],petal='hi_primrose',centre='hi_orange')
daisy_like('marsh_marigold',6,3.2,3.2,1.2,[(11,12,1.0,-2),(20,10,1.0,2),(16,17,0.9,0)],leafpal='hi_leaf_wet',petal='hi_butter',centre='hi_orange')
daisy_like('forget_me_not',5,1.9,2.0,0.8,[(9,13,1,-3),(13,9,1,-1),(18,8,1,1),(22,12,1,3),(15,14,1,0),(20,17,1,2),(11,18,1,-2)],petal='hi_forget',centre='hi_gold_centre')
daisy_like('edelweiss',7,3.6,2.4,1.8,[(15,11,1.0,0),(21,16,0.7,2)],leafpal='hi_wool_leaf',petal='hi_felt',centre='hi_felt_centre')
daisy_like('saxifrage',5,2.2,2.2,0.8,[(10,18,1,-3),(14,15,1,-1),(19,16,1,1),(23,19,1,3),(16,20,1,0)],leafpal='hi_leaf_blue',petal='hi_saxi',centre='hi_gold_centre')
daisy_like('desert_bloom',9,3.6,2.0,1.4,[(12,13,1.0,-2),(21,11,0.9,2)],leafpal='hi_sage_leaf',petal='hi_desert',centre='hi_gold_centre')
daisy_like('sunflower',20,6.4,2.4,3.2,[(15,9,1.0,0)],petal='hi_butter',centre='hi_seed_centre')

def spike_flower(name, spikes, w=2.6, leafpal='hi_leaf', bell_spike=False, **pal):
    def f():
        c=Canvas(); leaves_base(c,16,30,6,8,2.2,seed=len(name),sh=3)
        for i,(x,ytop,b) in enumerate(spikes):
            stem_to(c,16,x,ytop+10,b)
            floret_spike(c,x,ytop+10,ytop,w,'P',2,seed=i+len(name))
        rim(c); return c
    F[name]=(f,dict(leaf=leafpal,**pal))
spike_flower('lupine',[(12,3,-2),(20,6,2),(16,9,0)],3.6,petal='hi_lupine')
spike_flower('fireweed',[(13,2,-1),(20,7,2)],3.0,petal='hi_fireweed')
spike_flower('goldenrod',[(10,5,-3),(15,2,0),(21,4,3)],3.4,petal='hi_goldenrod')
spike_flower('foxglove',[(15,2,0)],4.8,petal='hi_foxglove')
spike_flower('heather',[(9,12,-3),(13,9,-1),(18,10,1),(22,13,3)],2.4,leafpal='hi_heath_leaf',petal='hi_heather')
spike_flower('thyme',[(10,16,-3),(15,14,0),(20,16,3)],2.4,leafpal='hi_heath_leaf',petal='hi_thyme')

@reg('clover',leaf='hi_leaf',petal='hi_clover')
def clover():
    c=Canvas()
    for x,y in [(9,25),(14,23),(20,24),(24,27),(12,28)]:
        for a in (-90,30,150): petal(c,x,y,a,2.4,2.6,'L',3)
    for x,y,b in [(12,13,-2),(20,15,2)]:
        stem_to(c,16,x,y+2,b)
        disc(c,x,y,3.0,'P',2)
        for i in range(10):
            a=math.radians(i*36); c.put(x+math.cos(a)*2,y+math.sin(a)*2,'P',3 if i<5 else 1)
    rim(c); return c
@reg('yarrow',leaf='hi_fern_leaf',petal='hi_white')
def yarrow():
    c=Canvas(); leaves_base(c,16,30,6,8,1.6,seed=4)
    for x,y,b in [(11,8,-2),(18,6,1),(22,11,3)]:
        stem_to(c,16,x,y+3,b)
        for dx in range(-4,5):
            for dy in range(-2,2):
                if abs(dx)+abs(dy)*1.6<5.2: c.put(x+dx,y+dy,'P',(3 if dy<0 else 2)+(1 if (dx+dy)%3==0 else 0)-(1 if dx>2 else 0))
        for dx in (-2,0,2): line(c,x+dx,y+1,x,y+3,'L',2)
    rim(c); return c
@reg('thistle',leaf='hi_thistle_leaf',petal='hi_thistle')
def thistle():
    c=Canvas()
    for x,a in [(13,-150),(19,-30),(14,-120),(18,-60)]: blade_leaf(c,x,26,a,7,3.4,3,droop=0.3)
    for x,y,b in [(14,9,-1),(21,13,2)]:
        stem_to(c,16,x,y+4,b)
        disc(c,x,y+3,2.4,'L',2)
        for k in range(5): c.put(x-2+k,y+3-(k%2),'L',3)
        for dx in range(-3,4):
            for dy in range(-3,1):
                if abs(dx)<=3+dy: c.put(x+dx,y+dy,'P',2+(1 if dx<0 else 0)+(1 if dy<-2 else 0))
    rim(c); return c
@reg('iris',leaf='hi_leaf_blue',petal='hi_iris',centre='hi_gold_centre')
def iris():
    c=Canvas()
    for x,a,d in [(13,-100,-0.2),(15,-95,0),(18,-82,0.2),(20,-75,0.3)]: blade_leaf(c,x,31,a,17,2.4,3,droop=d)
    stem_to(c,16,16,13,0,2)
    cx,cy=16,9
    petal(c,cx,cy,-90,5,3.4,'P',3); petal(c,cx,cy,-130,4,2.6,'P',3); petal(c,cx,cy,-50,4,2.6,'P',2)
    petal(c,cx,cy+1,150,5,3.2,'P',1); petal(c,cx,cy+1,30,5,3.2,'P',1)
    c.put(cx-3,cy+2,'C',2); c.put(cx+3,cy+2,'C',1)
    rim(c); return c
@reg('snowdrop',leaf='hi_leaf_blue',petal='hi_white')
def snowdrop():
    c=Canvas()
    for x,a,d in [(14,-100,-0.3),(18,-80,0.3)]: blade_leaf(c,x,31,a,13,2.2,3,droop=d)
    for (x0,xt,yt,bx,by) in [(15,13,9,10,13),(17,20,12,22,16)]:
        stem(c,curve(x0,31,xt,yt,0),2); line(c,xt,yt,bx,by-3,'L',2)
        bell(c,bx,by,2,'P',3); c.put(bx,by-3,'L',3)
    rim(c); return c
@reg('crocus',leaf='hi_leaf',petal='hi_crocus',centre='hi_orange')
def crocus():
    c=Canvas()
    for x,a,d in [(13,-110,-0.3),(16,-92,0),(19,-72,0.3)]: blade_leaf(c,x,31,a,9,1.4,4,droop=d)
    for x,y in [(12,22),(20,21)]:
        cup(c,x,y,5,7,'P',2,lobes=2); c.put(x,y-3,'C',2); c.put(x,y-4,'C',1)
        line(c,x,y+1,x,30,'L',4)
    rim(c); return c
@reg('gentian',leaf='hi_leaf',petal='hi_gentian')
def gentian():
    c=Canvas(); leaves_base(c,16,30,6,5,2.2,seed=8,sh=3)
    for x,y in [(11,17),(17,14),(22,18)]:
        line(c,x,y+2,16,29,'L',2)
        cup(c,x,y,5,8,'P',2,lobes=5)
    rim(c); return c
@reg('peony',leaf='hi_leaf',petal='hi_peony',centre='hi_gold_centre')
def peony():
    c=Canvas(); leaves_base(c,16,30,6,8,3.4,seed=12,droop=0.5)
    stem_to(c,16,15,13,-1)
    cx,cy=15,9
    for ring,(n,l,w,s) in enumerate([(10,6.2,4.2,1),(8,4.6,3.8,2),(6,3.0,3.2,3)]):
        for i in range(n):
            a=i*360/n+ring*17
            petal(c,cx,cy,a,l,w,'P',s+(1 if math.sin(math.radians(a))<-0.3 else 0))
    c.put(cx-1,cy-1,'P',4); c.put(cx,cy-2,'P',4)
    rim(c); return c
@reg('sweet_violet',leaf='hi_leaf',petal='hi_violet',centre='hi_gold_centre')
def violet():
    c=Canvas()
    for x,y in [(10,26),(16,24),(22,26),(13,29),(19,29)]: disc(c,x,y,2.8,'L',3)
    for x,y,b in [(11,16,-2),(20,14,2),(16,19,0)]:
        stem_to(c,16,x,y+2,b)
        petal(c,x,y,-120,2.8,2.6,'P',3); petal(c,x,y,-60,2.8,2.6,'P',2)
        petal(c,x,y,170,2.4,2.4,'P',3); petal(c,x,y,10,2.4,2.4,'P',2); petal(c,x,y,90,3,2.8,'P',1)
        c.put(x,y,'C',2)
    rim(c); return c
@reg('columbine',leaf='hi_fern_leaf',petal='hi_columbine',centre='hi_gold_centre',second='hi_white')
def columbine():
    c=Canvas(); leaves_base(c,16,30,5,6,2.6,seed=13)
    for x,y,b in [(11,10,-2),(21,12,2)]:
        stem_to(c,16,x,y-2,b)
        for a in (-150,-110,-70,-30,10,170): petal(c,x,y,a,4.0,1.8,'P',2)
        for a in (-120,-60,90): petal(c,x,y+1,a,2.4,2.2,'S',3)
        c.put(x,y+2,'C',2)
    rim(c); return c
@reg('bellflower',leaf='hi_leaf',petal='hi_bell')
def bellflower():
    c=Canvas(); leaves_base(c,16,30,5,6,2.4,seed=14)
    stem(c,[(16,31),(16,22),(17,14),(18,7)],2)
    for (x,y) in [(13,11),(20,15),(13,20)]:
        line(c,17,y-3,x,y-3,'L',2)
        cup(c,x,y+3,5,6,'P',2,lobes=5) if False else bell(c,x,y,2,'P',2)
    rim(c); return c
@reg('hibiscus',leaf='hi_leaf_dark',petal='hi_hibiscus',centre='hi_gold_centre')
def hibiscus():
    c=Canvas(); leaves_base(c,16,30,6,7,3.6,seed=15)
    stem_to(c,16,16,12,0)
    cx,cy=16,10
    for i in range(5): petal(c,cx,cy,-90+i*72,6,5.6,'P',2+(1 if i in (0,4) else 0))
    line(c,cx,cy,cx+4,cy-4,'C',1); c.put(cx+4,cy-5,'C',2); c.put(cx+5,cy-4,'C',2)
    disc(c,cx,cy,1.2,'P',0,shade=False)
    rim(c); return c
