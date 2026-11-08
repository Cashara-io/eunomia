import csv,json
from decimal import Decimal as D
from pathlib import Path
s=Path('/private/tmp/eunomia-gate3')
def money(v):return D(v.replace('$','').replace(',','')) if v else None
rows=[]
for r in csv.DictReader((s/'materials/stock.csv').open()):
 rows.append(dict(name=r['item'],supplier=r['supplier'].casefold(),price=money(r['sell price AUD']),cost=money(r['unit cost AUD']),qty=D(r['qty on hand'])))
def values(rr):
 good=[r for r in rr if r['cost'] is not None]
 cost=sum(r['cost']*r['qty'] for r in good);retail=sum(r['price']*r['qty'] for r in good)
 return {'rows':len(rr),'costed_rows':len(good),'average_sell_price':sum(r['price'] for r in rr)/D(len(rr)),'total_stock_value_at_cost':cost,'retail_all_rows':sum(r['price']*r['qty'] for r in rr),'retail_costed_rows':retail,'gross_margin_stock_weighted':sum((r['price']-r['cost'])*r['qty'] for r in good)/retail,'gross_margin_unweighted':sum(r['price']-r['cost'] for r in good)/sum(r['price'] for r in good),'average_unit_margin':sum(r['price']-r['cost'] for r in good)/D(len(good))}
baseline=values(rows);fronius=values([r for r in rows if r['supplier']=='fronius australia'])
changed=[dict(r,cost=r['cost']*D('1.1') if r['supplier']=='rexel darwin' and r['cost'] is not None else r['cost']) for r in rows]
after=values(changed)
out={'overall':baseline,'fronius':fronius,'scenario_rexel_plus_10pct':after,'scenario_deltas':{k:after[k]-baseline[k] for k in baseline if isinstance(baseline[k],D)},'rexel_changed_rows':[{'name':a['name'],'before':a['cost'],'after':b['cost'],'delta':b['cost']-a['cost']} for a,b in zip(rows,changed) if a['supplier']=='rexel darwin' and a['cost'] is not None]}
(s/'ev/recompute.json').write_text(json.dumps(out,indent=2,default=str));print(json.dumps(out,indent=2,default=str))
