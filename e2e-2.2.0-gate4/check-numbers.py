import json,runpy,csv,collections,datetime
from decimal import Decimal as D
from pathlib import Path
s=Path('/private/tmp/eunomia-gate4');h=runpy.run_path(str(s/'harness.py'));o=json.loads((s/'ev/ontology-approved.json').read_text());rows=[]
for r in csv.DictReader((s/'materials/catalogue.csv').open()):
 rows.append({'name':r['product'],'supplier':r['supplier'].casefold(),'sell_price':D(r['sell price AUD'].replace('$','').replace(',','')),'unit_cost':D(r['unit cost AUD'].replace('$','').replace(',','')) if r['unit cost AUD'] else None,'qty_on_hand':D(r['qty on hand']),'status':r['status']})
def row(expr,r):
 if isinstance(expr,str):return r.get(expr)
 if isinstance(expr,(int,float)):return D(str(expr))
 a=row(expr['left'],r);b=row(expr['right'],r)
 if a is None or b is None:return None
 return {'mul':lambda:a*b,'sub':lambda:a-b,'add':lambda:a+b,'div':lambda:a/b if b else None}[expr['op']]()
def evalexpr(expr,rs):
 for f in expr.get('filters',[]):rs=[r for r in rs if r.get(f['property'])==f['value']]
 agg=expr['agg']
 if agg=='count':return D(len(rs))
 if agg=='ratio':
  a=expr['numerator'];b=expr['denominator'];rs=[r for r in rs if evalexpr(a,[r]) is not None and evalexpr(b,[r]) is not None]
  av=evalexpr(a,rs);bv=evalexpr(b,rs);return av/bv if bv else None
 vals=[row(expr.get('row',expr.get('property')),r) for r in rs];vals=[v for v in vals if v is not None]
 if not vals:return None
 return sum(vals) if agg=='sum' else sum(vals)/D(len(vals)) if agg=='avg' else min(vals) if agg=='min' else max(vals)
metrics={(k['kind'],m['name']):m for k in o['kinds'] for m in k['metrics']}
checks=[]
def check(label,kind,name,actual,rs,unit=None):
 m=metrics[(kind,name)];expected=evalexpr(m['expr'],rs)
 if unit=='%' or m.get('unit') in ('percent','%'):expected*=100
 diff=abs(D(str(actual))-expected) if actual is not None and expected is not None else D(999)
 checks.append({'label':label,'kind':kind,'metric':name,'actual':actual,'recomputed':str(expected),'difference':str(diff),'pass':diff<=D('.01')})
for (kind,name),m in metrics.items():
 if m['status']!='approved':continue
 r=h['mcp']('metric_eval',{'vault_id':'Fremantle Fish & Ice Co','kind':kind,'metric':name});h['save']('ev/metric-'+name+'.json',r);check('direct',kind,name,r.get('value'),rows if kind=='product' else [{}]*6,r.get('unit'))
for f in (s/'p3').glob('*.tools.json'):
 for t in json.loads(f.read_text()):
  r=t.get('result',{})
  if t['tool'].endswith('metric_eval') and isinstance(r,dict) and 'value'in r:
   rr=[x for x in rows if x['supplier']=='shark bay prawns'] if 'related_to' in t['arguments'] else rows
   check(f.stem,r['kind'],r['metric'],r['value'],rr,r.get('unit'))
  if t['tool'].endswith('scenario_run') and isinstance(r,dict) and 'baseline'in r:
   changed=[dict(x,unit_cost=x['unit_cost']*D('1.1') if x['supplier']=='southern ice works' and x['unit_cost'] is not None else x['unit_cost']) for x in rows]
   for a in r['baseline']:check(f.stem+' baseline',a['kind'],a['metric'],a['value'],rows,a.get('unit'))
   for a in r['scenario']:check(f.stem+' scenario',a['kind'],a['metric'],a['value'],changed,a.get('unit'))
chat=json.loads((s/'ev/chat-final.json').read_text());calls={c['id']:c['function']['name'] for msg in chat[0]['result'] for c in msg.get('tool_calls',[])}
for msg in chat[0]['result']:
 if msg['role']!='tool':continue
 try:r=json.loads(msg['content'])
 except:continue
 tool=calls.get(msg.get('tool_call_id'))
 if tool=='metric_eval':check('chat',r['kind'],r['metric'],r['value'],rows,r.get('unit'))
 if tool=='scenario_run':
  changed=[dict(x,unit_cost=x['unit_cost']*D('1.1') if x['supplier']=='southern ice works' and x['unit_cost'] is not None else x['unit_cost']) for x in rows]
  for a in r['baseline']:check('chat baseline',a['kind'],a['metric'],a['value'],rows,a.get('unit'))
  for a in r['scenario']:check('chat scenario',a['kind'],a['metric'],a['value'],changed,a.get('unit'))
h['save']('ev/numbers-check.json',checks)
timing=[]
for t in chat[1]['result']:
 ms=sorted([m for m in chat[0]['result'] if m['thread_id']==t['id']],key=lambda x:x['created_at'])
 def dt(x):return datetime.datetime.fromisoformat(x.replace('Z','+00:00'))
 timing.append({'question':t['title'],'seconds':(dt(ms[-1]['created_at'])-dt(ms[0]['created_at'])).total_seconds()})
h['save']('ev/chat-timings-verified.json',timing)
print(json.dumps({'checks':len(checks),'failures':[x for x in checks if not x['pass']],'chat_timings':timing},indent=2))
