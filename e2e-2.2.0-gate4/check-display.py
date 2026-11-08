import runpy,json,csv
from pathlib import Path
from decimal import Decimal as D,ROUND_HALF_UP
s=Path('/private/tmp/eunomia-gate4');d=runpy.run_path(str(s/'recompute.py'));rows=d['rs'];calc=d['calc'];before=calc(rows);after=calc(d['changed']);checks=[]
def check(label,a,b):
 a=D(str(a));b=D(str(b));diff=abs(a-b);checks.append({'label':label,'actual':str(a),'independent':str(b),'difference':str(diff),'pass':diff<=D('.01')})
def equal(label,a,b):
 check(label,a,b);checks[-1]['pass']=D(str(a))==D(str(b));checks[-1]['rule']='exact equality of displayed decimals'
check('MCP Q1 average 130.79',130.79,before['average_sell_price']);check('MCP Q1 cost 27424.50',27424.5,before['total_stock_at_cost']);check('MCP Q2 margin 35.7%',35.7,calc([r for r in rows if r['supplier']=='Shark Bay Prawns'])['gross_margin_unweighted_pct'])
basecost=sum(r['unit cost AUD'] for r in rows if r['unit cost AUD'] is not None)/28
newcost=sum(r['unit cost AUD'] for r in d['changed'] if r['unit cost AUD'] is not None)/28
for label,a,b in [('MCP Q4 baseline margin',35.76,before['gross_margin_unweighted_pct']),('MCP Q4 scenario margin',35.64,after['gross_margin_unweighted_pct']),('MCP Q4 margin delta',-.12,after['gross_margin_unweighted_pct']-before['gross_margin_unweighted_pct']),('MCP Q4 cost before',27424.5,before['total_stock_at_cost']),('MCP Q4 cost after',27639.88,after['total_stock_at_cost']),('MCP Q4 cost delta',215.38,after['total_stock_at_cost']-before['total_stock_at_cost']),('MCP Q4 avg cost before',85.18,basecost),('MCP Q4 avg cost after',85.34,newcost)]:check(label,a,b)
for label,b,a,delta,exactb,exacta in [('UI avg cost','85.180','85.335','.155',basecost,newcost),('UI gross margin','35.76','35.64','-.12',before['gross_margin_unweighted_pct'],after['gross_margin_unweighted_pct']),('UI stock cost','27424.50','27639.88','215.38',before['total_stock_at_cost'],after['total_stock_at_cost'])]:
 check(label+' before',b,exactb);check(label+' after',a,exacta);check(label+' delta',delta,exacta-exactb);equal(label+' shown end minus start',D(a)-D(b),delta)
equal('MCP Q4 margin shown endpoints subtract to stated delta',D('35.64')-D('35.76'),'-.12');equal('MCP Q4 money shown endpoints subtract to stated delta',D('27639.88')-D('27424.50'),'215.38')
check('UI avg cost relative change 0.2%',D('.2'),(newcost-basecost)/basecost*100);check('UI stock cost relative change 0.8%',D('.8'),(after['total_stock_at_cost']-before['total_stock_at_cost'])/before['total_stock_at_cost']*100)
# Raw scenario amounts, exact deltas and scaled per-row costs are independently derived.
for folder in ['p3','chat']:
 if folder=='p3':results=[t['result'] for t in json.load(open(s/'p3/q4.tools.json')) if t['tool'].endswith('scenario_run')]
 else:
  results=[]
  for m in json.load(open(s/'ev/chat-final.json'))[0]['result']:
   if m['role']=='tool':
    try:r=json.loads(m['content'])
    except:continue
    if isinstance(r,dict) and 'baseline' in r:results.append(r)
 for result in results:
  baseline={m['metric']:m['value'] for m in result['baseline']}
  for m in result['scenario']:
   expected={'gross_margin':after['gross_margin_unweighted_pct']-before['gross_margin_unweighted_pct'],'avg_unit_cost':newcost-basecost,'total_stock_value_at_cost':after['total_stock_at_cost']-before['total_stock_at_cost']}[m['metric']]
   check(folder+' exact delta '+m['metric'],m['delta'],expected)
   check(folder+' relative delta '+m['metric'],m['delta_pct'],expected/D(str(baseline[m['metric']]))*100)
  for change in result['changed']:
   row=next(r for r in rows if r['product']==change['name']);check(folder+' changed before '+change['name'],change['before'],row['unit cost AUD']);check(folder+' changed after '+change['name'],change['after'],row['unit cost AUD']*D('1.1'))
for name,shown in [('Block Ice 20kg','7.98'),('Insulated Seafood Carton','7.43')]:
 exact=next(r for r in rows if r['product']==name)['unit cost AUD']*D('1.1');equal('UI half-away money '+name,shown,exact.quantize(D('.01'),rounding=ROUND_HALF_UP))
equal('UI/MCP half-away stock total','27639.88',after['total_stock_at_cost'].quantize(D('.01'),rounding=ROUND_HALF_UP));equal('UI/MCP half-away stock delta','215.38',(after['total_stock_at_cost']-before['total_stock_at_cost']).quantize(D('.01'),rounding=ROUND_HALF_UP))
(s/'ev/display-number-check.json').write_text(json.dumps(checks,indent=2));print(json.dumps({'checks':len(checks),'failures':[c for c in checks if not c['pass']]},indent=2))
