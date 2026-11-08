import json
from pathlib import Path
s=Path('/private/tmp/eunomia-gate4')
for f in sorted(s.glob('p*/*.jsonl')):
 out=[];calls={}
 for line in f.read_text().splitlines():
  try:d=json.loads(line)
  except:continue
  for c in d.get('message',{}).get('content',[]) if d.get('type') in ['user','assistant'] else []:
   if not isinstance(c,dict):continue
   if c.get('type')=='tool_use':
    v={'tool':c['name'],'arguments':c['input'],'id':c['id']};out.append(v);calls[c['id']]=v
   elif c.get('type')=='tool_result':
    texts=c.get('content',[]);v=calls.get(c['tool_use_id'])
    if v is not None:
     raw='\n'.join(x.get('text','') for x in texts if isinstance(x,dict)) if isinstance(texts,list) else texts
     try:v['result']=json.loads(raw)
     except:v['result']=raw
 f.with_suffix('.tools.json').write_text(json.dumps(out,indent=2,ensure_ascii=False))
print('Structured tool evidence extracted.')
