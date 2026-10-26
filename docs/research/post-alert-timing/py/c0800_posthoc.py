import json,glob,collections,random,sys
from datetime import datetime
from zoneinfo import ZoneInfo
ET=ZoneInfo("America/New_York")
def load(d):
    by=collections.defaultdict(dict)
    for f in glob.glob(d+"/*.jsonl"):
        for l in open(f):
            r=json.loads(l)
            if "p1" not in r.get("pops",[]) or not r.get("quoted"): continue
            by[r["strat"]][(r["day"],r["sym"],r["t0"])]=r
    return by
def is08(t0):
    t=datetime.fromtimestamp(t0/1e9,ET); return t.hour==8 and t.minute<2
def boot(pairs):
    d=collections.defaultdict(list)
    for day,x in pairs: d[day].append(x)
    dl=list(d); m=lambda s: sum(s)/len(s)
    pt=m([x for _,x in pairs]); bs=sorted(m([x for dd in (random.choice(dl) for _ in dl) for x in d[dd]]) for _ in range(1000))
    return f"{pt*100:+.2f} [{bs[25]*100:+.2f}, {bs[975]*100:+.2f}] n={len(pairs)}"
for name in ["rows-confirm","rows-explore-a2"]:
    by=load("C:/research/stockwatcher/work-pat/"+name)
    if "D8" not in by: print(name,"strats",list(by)[:20]); continue
    for lab,f in [("all",lambda k:True),("08:00-08:02",lambda k:is08(k[2])),("without 08:00",lambda k:not is08(k[2]))]:
        ks=[k for k in by["B0"] if k in by["D8"] and f(k)]
        net=lambda r:(r["net"] if r.get("entered") else 0.0)
        pairs=[(k[0],net(by["D8"][k])-net(by["B0"][k])) for k in ks]
        b0=[net(by["B0"][k]) for k in ks]
        print(name,lab,"D8-B0",boot(pairs),"B0 mean %.2f%%"%(100*sum(b0)/max(1,len(b0))))
