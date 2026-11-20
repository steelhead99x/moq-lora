#!/usr/bin/env python3
"""Raw logger for the stock Electronic Cats LoRa receiver sketch (stdlib only).
Writes every byte read from the stick with a monotonic timestamp to <out>.raw.jsonl
(base64 bytes) and parsed packets to <out>.packets.csv. Stop with SIGINT/SIGTERM or --seconds."""
import os, sys, time, termios, base64, json, re, signal, argparse, select
ap=argparse.ArgumentParser(); ap.add_argument('--dev',default='/dev/ttyACM0')
ap.add_argument('--out',required=True); ap.add_argument('--seconds',type=float,default=0); a=ap.parse_args()
fd=os.open(a.dev,os.O_RDWR|os.O_NOCTTY|os.O_NONBLOCK)
t=termios.tcgetattr(fd); t[0]=0;t[1]=0;t[2]=termios.CS8|termios.CREAD|termios.CLOCAL;t[3]=0
t[4]=t[5]=termios.B9600; t[6][termios.VMIN]=0;t[6][termios.VTIME]=0; termios.tcsetattr(fd,termios.TCSANOW,t)
stop=[False]
for s in (signal.SIGINT,signal.SIGTERM): signal.signal(s,lambda *x: stop.__setitem__(0,True))
raw=open(a.out+'.raw.jsonl','a'); csv=open(a.out+'.packets.csv','a')
if csv.tell()==0: csv.write('t_mono_s,t_epoch_s,rssi_dbm,len,payload_b64\n')
buf=b''; t0=time.monotonic(); pat=re.compile(rb"Received packet '(.*?)' with RSSI (-?\d+)\r?\n",re.S)
while not stop[0] and (not a.seconds or time.monotonic()-t0<a.seconds):
    r,_,_=select.select([fd],[],[],0.2)
    if not r: continue
    try: d=os.read(fd,4096)
    except BlockingIOError: continue
    now=time.monotonic(); ep=time.time()
    raw.write(json.dumps({'t':round(now,4),'e':round(ep,4),'b':base64.b64encode(d).decode()})+'\n'); raw.flush()
    buf+=d
    while True:
        m=pat.search(buf)
        if not m: break
        p=m.group(1); csv.write(f"{now:.4f},{ep:.4f},{m.group(2).decode()},{len(p)},{base64.b64encode(p).decode()}\n"); csv.flush()
        buf=buf[m.end():]
    if len(buf)>8192: buf=buf[-4096:]
