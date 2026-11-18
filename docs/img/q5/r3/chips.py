# chips.py <clip> <from> <to>: the worker chips' boxes per frame (1280x720): each chip's 1 px hair_t frame (45,49,56)
# top edge, found as a horizontal run of at least 200 px in y 500..640: printed as (x0, x1, y) per run, when they change.
import subprocess, sys, numpy as np
W,H=1280,720
path,a,b=sys.argv[1],int(sys.argv[2]),int(sys.argv[3])
p=subprocess.Popen(['ffmpeg','-loglevel','error','-i',path,'-f','rawvideo','-pix_fmt','rgb24','-'],stdout=subprocess.PIPE)
n=0; prev=None; out=[]
hair=np.array([45,49,56])
while True:
    buf=p.stdout.read(W*H*3)
    if len(buf)<W*H*3 or n>b: break
    if n>=a:
        f=np.frombuffer(buf,np.uint8).reshape(H,W,3).astype(int)
        m=np.all(np.abs(f-hair)<=2,axis=2)
        runs=[]
        for y in range(500,640):
            d=np.diff(np.concatenate([[0],m[y].astype(int),[0]]))
            s=np.where(d==1)[0]; e=np.where(d==-1)[0]
            runs+=[(int(x0),int(x1),y) for x0,x1 in zip(s,e) if x1-x0>=200]
        # a box's top edge: a run whose row above has no run at that x
        tops=[r for r in runs if not any(q[2]==r[2]-1 and abs(q[0]-r[0])<3 for q in runs)]
        key=tuple(tops)
        if key!=prev: out.append(f"f{n} {list(key)}")
        prev=key
    n+=1
p.kill()
print('\n'.join(out))
