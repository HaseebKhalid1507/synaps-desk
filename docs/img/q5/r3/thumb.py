# thumb.py <clip> <from> <to> <x0> <x1>: the transcript scrollbar's thumb wherever it is in x0..x1 (it rides the right
# panel's edge): per frame, the longest vertical run of thumb-bright pixels (R+G+B > 200, at least 60 px, 6 px wide),
# printed as (x, top, bottom) when it changes. The clip's size from ffprobe.
import subprocess, sys, numpy as np
path,a,b,x0,x1=sys.argv[1],int(sys.argv[2]),int(sys.argv[3]),int(sys.argv[4]),int(sys.argv[5])
W,H=map(int,subprocess.check_output(['ffprobe','-v','error','-select_streams','v:0','-show_entries','stream=width,height','-of','csv=p=0',path]).decode().strip().split(','))
p=subprocess.Popen(['ffmpeg','-loglevel','error','-i',path,'-f','rawvideo','-pix_fmt','rgb24','-'],stdout=subprocess.PIPE)
n=0; prev=None; out=[]
while True:
    buf=p.stdout.read(W*H*3)
    if len(buf)<W*H*3 or n>b: break
    if n>=a:
        f=np.frombuffer(buf,np.uint8).reshape(H,W,3).astype(int).sum(axis=2)[60:H-5,x0:x1]
        on=f>200
        best=None
        for x in range(on.shape[1]-6):
            col=on[:,x:x+6].all(axis=1)
            if not col.any(): continue
            d=np.diff(np.concatenate([[0],col.astype(int),[0]]))
            s=np.where(d==1)[0]; e=np.where(d==-1)[0]
            L=e-s; k=np.argmax(L)
            if L[k]>=60 and (best is None or L[k]>best[2]-best[1]): best=(x+x0,int(s[k])+60,int(e[k])+59)
        if best!=prev: out.append(f"f{n} {best}")
        prev=best
    n+=1
p.kill()
print(' | '.join(out))
