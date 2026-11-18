# rules.py <clip> <from> <to>: the rail's section rules (1 px, colour (28,32,40) at x 0, y 60..715) per frame, printed
# when they change: each section head's place, to follow what moves in a fold.
import subprocess, sys, numpy as np
path,a,b=sys.argv[1],int(sys.argv[2]),int(sys.argv[3])
W,H=1280,720
p=subprocess.Popen(['ffmpeg','-loglevel','error','-i',path,'-f','rawvideo','-pix_fmt','rgb24','-'],stdout=subprocess.PIPE)
n=0; prev=None; out=[]
while True:
    buf=p.stdout.read(W*H*3)
    if len(buf)<W*H*3 or n>b: break
    if n>=a:
        col=np.frombuffer(buf,np.uint8).reshape(H,W,3)[60:716,0].astype(int)
        ys=tuple(int(y)+60 for y in np.where(np.all(col==[28,32,40],axis=1))[0])
        if ys!=prev: out.append(f"f{n} {','.join(map(str,ys))}")
        prev=ys
    n+=1
p.kill()
print(' | '.join(out))
