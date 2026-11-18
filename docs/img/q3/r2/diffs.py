import subprocess, numpy as np, sys
from scipy import ndimage
f=sys.argv[1]; W,H=map(int,sys.argv[2].split('x')); out=open(sys.argv[3],'w')
p=subprocess.Popen(['ffmpeg','-loglevel','error','-i',f,'-f','rawvideo','-pix_fmt','rgb24','-'],stdout=subprocess.PIPE)
prev=None; n=0
while True:
    b=p.stdout.read(W*H*3)
    if len(b)<W*H*3: break
    im=np.frombuffer(b,np.uint8).reshape(H,W,3)
    if prev is not None:
        d=(np.abs(im.astype(np.int16)-prev.astype(np.int16)).sum(axis=2)>0)
        if d.any():
            lab,k=ndimage.label(ndimage.binary_dilation(d,iterations=3))
            boxes=[]
            for sl in ndimage.find_objects(lab):
                ys,xs=sl; boxes.append(f"{xs.start},{ys.start}-{xs.stop},{ys.stop}")
            out.write(f"{n} {int(d.sum())} {' '.join(boxes)}\n")
    prev=im; n+=1
print(n)
