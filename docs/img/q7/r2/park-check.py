import sys, subprocess, numpy as np
f=sys.argv[1]; W,H=1280,720
p=subprocess.Popen(['ffmpeg','-loglevel','error','-i',f,'-f','rawvideo','-pix_fmt','rgb24','-'],stdout=subprocess.PIPE)
n=0; prev=None; rows=[]
while True:
    b=p.stdout.read(W*H*3)
    if len(b)<W*H*3: break
    a=np.frombuffer(b,np.uint8).reshape(H,W,3).astype(int)
    band=a[86:113,336:960]; below=a[113:600,336:960]
    # band present: row 112 uniform and differs from canvas row 120? use uniformity of row 112 + row 86..111 bg
    rule=a[112,336:960]; uniform=(np.abs(rule-rule[0]).sum(1)==0).all()
    canvas=a[300,20]
    if prev is not None:
        db=(np.abs(band-prev[0]).sum(2)>0).sum(); dl=(np.abs(below-prev[1]).sum(2)>0).sum()
        rows.append((n,uniform,db,dl))
    prev=(band,below); n+=1
print('frames',n)
# parked frames: below unchanged except small (spinners elsewhere) -> dl small; band changes while scroll still
hits=[(i,db,dl) for i,u,db,dl in rows if u and db>0 and dl<400]
print('band up & band pixels changed while the list is parked (below changed < 400 px):',len(hits))
print(hits[:40])
up=sum(1 for r in rows if r[1]); print('frames with the band (uniform rule row 112):',up)
