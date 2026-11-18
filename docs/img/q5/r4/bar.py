# bar.py <clip> <from> <to>: the rail's warn bar (the row that needs you: relay-refresh), x 0, colour (235,203,139):
# per frame, its runs (top, bottom) in y 60..715, printed when they change; "none" when it isn't drawn.
import subprocess, sys, numpy as np
path,a,b=sys.argv[1],int(sys.argv[2]),int(sys.argv[3])
W,H=map(int,subprocess.check_output(['ffprobe','-v','error','-select_streams','v:0','-show_entries','stream=width,height','-of','csv=p=0',path]).decode().strip().split(','))
p=subprocess.Popen(['ffmpeg','-loglevel','error','-i',path,'-f','rawvideo','-pix_fmt','rgb24','-'],stdout=subprocess.PIPE)
n=0; prev=None; out=[]
while True:
    buf=p.stdout.read(W*H*3)
    if len(buf)<W*H*3 or n>b: break
    if n>=a:
        col=np.frombuffer(buf,np.uint8).reshape(H,W,3)[60:716,0].astype(int)
        on=np.all(col==[235,203,139],axis=1).astype(int)
        d=np.diff(np.concatenate([[0],on,[0]]))
        runs=tuple((int(s)+60,int(e)+59) for s,e in zip(np.where(d==1)[0],np.where(d==-1)[0]))
        if runs!=prev: out.append(f"f{n} {' '.join(f'{s}-{e}' for s,e in runs) or 'none'}")
        prev=runs
    n+=1
p.kill()
print(' | '.join(out))
