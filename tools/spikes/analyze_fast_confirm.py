#!/usr/bin/env python3
"""Parse archived private logs, joined by durable request and native pane."""
import collections, datetime, gzip, json, math, re
from pathlib import Path
ROOT=Path('/Users/dylanliu/work/bus/temp/spikes/codexspikeb')
PAIR=re.compile(r'(?<![\w:])([A-Za-z_]+)=("(?:[^"\\]|\\.)*"|[^\s{}]+)')
def fields(line):
    return {k:(json.loads(v) if v.startswith('"') else v.rstrip('}:')) for k,v in PAIR.findall(line)}
def at(line):return datetime.datetime.fromisoformat(line.split()[0].replace('Z','+00:00')).timestamp()*1000
def percentile(values,p):
    values=sorted(values);return round(values[max(0,math.ceil(len(values)*p)-1)],3) if values else None
def distribution(values):return {'n':len(values),**{k:percentile(values,p) for k,p in [('p50',.5),('p90',.9),('p99',.99),('max',1)]}}
def readlines(path):
    data=gzip.decompress(path.read_bytes()).decode(errors='replace') if path.suffix=='.gz' else path.read_text(errors='replace')
    return data.splitlines()
def analyze(mode):
    folder=ROOT/mode; samples=json.loads((folder/'samples.json').read_text());audit=json.loads((folder/'fake-audit.json').read_text()); reqs=collections.defaultdict(dict);controls={};steering_callbacks=[]
    for file in folder.glob('log-*-herdr-client.log*'):
        for line in readlines(file):
            if not line.startswith('2026-'):continue
            f=fields(line);ev=f.get('event');t=at(line)
            if ev=='bus.control.command.dequeued' and f.get('method')=='message.send':controls[f['control_request_id']]=t
            rid=f.get('request_id','');rid=int(rid) if rid.isdigit() else None
            if ev=='bus.message.queued' and rid:
                r=reqs[rid];r['queued']=t;r['agent']=f.get('agent_id');r['accepted']=controls.get(f.get('control_request_id'));r['pane']=f.get('pane_id','').removeprefix('Some(').strip(')"')
            elif ev=='bus.delivery.start' and rid:reqs[rid]['picked']=t;reqs[rid]['reason']=f.get('reason')
            elif ev=='bus.callback.correlated' and f.get('kind')=='started' and f.get('disposition','').startswith('Accepted'):
                m=re.search(r'bus.callback\{[^}]*request_id=Some\((\d+)\)',line)
                if f.get('disposition')=='AcceptedSteering':steering_callbacks.append((t,f.get('agent_id')))
                elif m:reqs[int(m[1])].setdefault('started',t)
    for t,agent in steering_callbacks:
        candidates=[(r['picked'],rid) for rid,r in reqs.items() if r.get('agent')==agent and r.get('reason')=='steer' and r.get('picked',float('inf'))<=t]
        if candidates:reqs[max(candidates)[1]].setdefault('started',t)
    bypane=collections.defaultdict(list)
    for rid,r in reqs.items():
        if 'picked' in r:bypane[r['pane']].append((r['picked'],rid))
    for pane in bypane:bypane[pane].sort()
    for file in folder.glob('log-*-herdr-server.log*'):
        for line in readlines(file):
            if not line.startswith('2026-'):continue
            f=fields(line);ev=f.get('event');pane=f.get('pane_id');t=at(line)
            if not pane or not ev:continue
            candidates=[(picked,rid) for picked,rid in bypane[pane] if picked<=t]
            if not candidates:continue
            r=reqs[candidates[-1][1]]
            if ev in ['bus.spike.enter_flushed','bus.terminal.submission.enter_flushed'] and f.get('flushed')=='true':r.setdefault('entered',t)
            if ev=='bus.terminal.submission.paste':r.setdefault('paste_begin',t)
            if ev=='bus.terminal.submission.paste_flushed' and f.get('flushed')=='true':r.setdefault('box',t)
            if ev=='bus.terminal.submission.observed' and f.get('observation')=='Pending':r.setdefault('owned',t)
            if ev=='bus.terminal.submission.summary':r['summary']=f;r['confirmed']=t
    for row in samples:
        rid=row['request_ids'][0];r=reqs[rid];row['timing']=r
        body=row['text'].strip()
        expected=f"[Bus message {row['message_id']} from you]\n{row['text']}"
        enters=[a for a in audit if a['event']=='input' and a['data'].get('input')=='Enter' and a['data'].get('draft','')==expected and a['wall_ns']>=row['send_call_ns']]
        boxes=[a['wall_ns']/1e6 for a in audit if a['event']=='draft' and a['data'].get('draft')==expected and a['wall_ns']>=row['send_call_ns']]
        if boxes:r['fake_box']=min(boxes)
        row['consumed_enters']=len(enters)
        row['payload_byte_exact']=bool(len(enters)==1 and enters[0]['data']['draft'].split('\n',1)[1]==row['text'])
        row['settled_replied']=bool(row.get('status',{}).get('complete') and all(x.get('stage')=='replied' for x in row.get('status',{}).get('requests',[])))
        row['correct']=row['settled_replied'] and len(enters)==1 and row['payload_byte_exact']
        if row['scenario']=='S2':
            original=json.loads((folder/'setup.json').read_text())['draft']
            snapshots=[a for a in audit if a['event']=='draft' and a['data'].get('draft')==original]
            if snapshots:
                last=snapshots[-1]; later=[a for a in audit if a['pid']==last['pid'] and a['wall_ns']>last['wall_ns']]
                row['developer_draft_retained_byte_exact']=not later
            row['next_delivery_blocked']=any(s['scenario']=='S2-next' and s.get('timed_out') for s in samples)
            submitted=[a for a in audit if a['event']=='submit' and a['data'].get('prompt','').strip().endswith(body) and a['wall_ns']>=row['send_call_ns']]
            draft=json.loads((folder/'setup.json').read_text())['draft'];row['restored_byte_exact']=bool(len(submitted)==1 and submitted[0]['data']['draft']==draft);row['correct'] &= row['restored_byte_exact']
    intervals=[];opened=None
    for a in audit:
        if a['event']=='dialog_open':opened=a['wall_ns']/1e6
        elif a['event']=='dialog_close' and opened is not None:intervals.append([opened,a['wall_ns']/1e6]);opened=None
    wrong=[{'request':rid,'entered':r['entered']} for rid,r in reqs.items() if 'entered' in r and any(lo<=r['entered']<=hi for lo,hi in intervals)]
    results={}
    phases=[('accepted_to_enter','accepted','entered'),('accepted_to_started','accepted','started'),('accepted_to_box','accepted','box'),('accepted_to_fake_box','accepted','fake_box'),('accepted_to_pick','accepted','picked'),('paste_write','paste_begin','box'),('paste_to_owned','box','owned'),('owned_to_enter','owned','entered'),('enter_to_confirm','entered','confirmed')]
    for scenario in ['S1','S2','S3','S4','S5']:
        rows=[row for row in samples if row['scenario']==scenario];out={'n':len(rows),'correct':bool(rows) and (len(rows)==80 if scenario=='S5' else True) and all(row['correct'] for row in rows) and (not wrong if scenario=='S4' else True),'failures':[row['message_id'] for row in rows if not row['correct']]}
        for name,first,last in phases:out[name]=distribution([row['timing'][last]-row['timing'][first] for row in rows if row['timing'].get(first) is not None and last in row['timing']])
        results[scenario]=out
    result={'mode':mode,'scenarios':results,'wrong_enters_in_dialog':wrong,'dialog_intervals_ms':intervals,'samples':samples}
    (folder/'analysis.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n');return result
def resummarize(scenario, rows, wrong):
    rows=[r for r in rows if r['scenario']==scenario]
    phases=[('accepted_to_enter','accepted','entered'),('accepted_to_started','accepted','started'),('accepted_to_box','accepted','box'),('accepted_to_fake_box','accepted','fake_box'),('accepted_to_pick','accepted','picked'),('paste_write','paste_begin','box'),('paste_to_owned','box','owned'),('owned_to_enter','owned','entered'),('enter_to_confirm','entered','confirmed')]
    result={'n':len(rows),'correct':bool(rows) and (len(rows)==80 if scenario=='S5' else True) and all(row['correct'] for row in rows) and (not wrong if scenario=='S4' else True),'failures':[row['message_id'] for row in rows if not row['correct']]}
    for name,first,last in phases:result[name]=distribution([row['timing'][last]-row['timing'][first] for row in rows if row['timing'].get(first) is not None and last in row['timing']])
    return result

if __name__=='__main__':
    parts={mode:analyze(mode) for mode in ['before','after','after-tail','before-parallel','after-parallel'] if (ROOT/mode/'samples.json').exists() and (ROOT/mode/'fake-audit.json').exists()}
    for name,part in parts.items():
        for row in part['samples']:row['evidence_part']=name
    out={name:parts[name] for name in ['before','after'] if name in parts}
    if 'before-parallel' in parts and sum(r['scenario']=='S5' for r in parts['before-parallel']['samples'])==50:
        out['before']['parts']=['before','before-parallel']
        out['before']['samples']=[r for r in out['before']['samples'] if not(r['scenario']=='S5' and 'parallel' in r['text'])]+parts['before-parallel']['samples']
    for part in ['after-tail','after-parallel']:
        if part in parts and 'after' in out:
            target=out['after'];extra=parts[part]
            target.setdefault('parts',['after']).append(part)
            target['samples'].extend(extra['samples'])
            target['wrong_enters_in_dialog'].extend(extra['wrong_enters_in_dialog'])
            target['dialog_intervals_ms'].extend(extra['dialog_intervals_ms'])
    for mode,result in out.items():
        for scenario in ['S1','S2','S3','S4','S5']:result['scenarios'][scenario]=resummarize(scenario,result['samples'],result['wrong_enters_in_dialog'])
    (ROOT/'comparison.json').write_text(json.dumps(out,ensure_ascii=False,indent=2)+'\n')
    for mode,result in out.items():
        print(mode)
        for k,v in result['scenarios'].items():print(k,'PASS' if v['correct'] else 'FAIL',v['accepted_to_enter'], 'started',v['accepted_to_started'])
