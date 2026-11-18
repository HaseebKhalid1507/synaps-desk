# thumb2.py <clip> <from> <to>: the transcript scrollbar's thumb wherever it is (it rides the right panel's edge):
# per frame, the longest vertical run of thumb-bright pixels (R+G+B > 200, at least 60 px, 6 px wide) in x 900..1279,
# printed as (x, top, bottom) when it changes.
import subprocess, sys, numpy as np
W,H=1280,720
path,a,b=sys.argv[1],int(sys.argv[2]),int(sys.argv[3])
p=subprocess.Popen(['ffmpeg','-loglevel','error','-i',path,'-f','rawvideo','-pix_fmt','rgb24','-'],stdout=subprocess.PIPE)
n=0; prev=None; out=[]
while True:
    buf=p.stdout.read(W*H*3)
    if len(buf)<W*H*3 or n>b: break
    if n>=a:
        f=np.frombuffer(buf,np.uint8).reshape(H,W,3).astype(int).sum(axis=2)[60:715,900:1280]
        on=f>200
        best=None
        for x in range(on.shape[1]-6):
            col=on[:,x:x+6].all(axis=1)
            if not col.any(): continue
            d=np.diff(np.concatenate([[0],col.astype(int),[0]]))
            s=np.where(d==1)[0]; e=np.where(d==-1)[0]
            L=e-s; k=np.argmax(L)
            if L[k]>=60 and (best is None or L[k]>best[2]-best[1]): best=(x+900,int(s[k])+60,int(e[k])+59)
        t=best
        if t!=prev: out.append(f"f{n} {t}")
        prev=t
    n+=1
p.kill()
print(' | '.join(out))
