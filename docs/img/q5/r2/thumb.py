# thumb.py <clip> <x> <from> <to>: the transcript scrollbar thumb's rows (top, bottom) in column x, per frame, when they change.
import subprocess, sys, numpy as np
W,H=1280,720
path,x,a,b=sys.argv[1],int(sys.argv[2]),int(sys.argv[3]),int(sys.argv[4])
p=subprocess.Popen(['ffmpeg','-loglevel','error','-i',path,'-f','rawvideo','-pix_fmt','rgb24','-'],stdout=subprocess.PIPE)
n=0; prev=None; out=[]
while True:
    buf=p.stdout.read(W*H*3)
    if len(buf)<W*H*3 or n>b: break
    if n>=a:
        f=np.frombuffer(buf,np.uint8).reshape(H,W,3)
        col=f[60:715,x].astype(int).sum(axis=1)
        on=np.where(col>150)[0]
        t=(int(on[0])+60,int(on[-1])+60) if len(on) else None
        if t!=prev: out.append(f"f{n} {t}")
        prev=t
    n+=1
p.kill()
print(' | '.join(out))
