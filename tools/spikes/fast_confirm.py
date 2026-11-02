#!/usr/bin/env python3
"""Throwaway fake-only comparison of master path vs event-driven confirmation."""
import concurrent.futures, hashlib, importlib.util, json, os, re, secrets, shutil, socket, subprocess, sys, tempfile, threading, time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('terminal', REPO/'tests/kimi_trust_acceptance_test.py')
terminal_module = importlib.util.module_from_spec(spec); spec.loader.exec_module(terminal_module)
EVIDENCE = Path('/Users/dylanliu/work/bus/temp/spikes/codexspikeb')
BINARY = Path('/private/tmp/codexspikeb-binary/bus')
PAYLOAD = Path('/Users/dylanliu/work/bus/temp/unsent-draft/review-reply-payload.txt').read_text()
DRAFT = 'developer draft\n  keep tabs\tand spaces  \n第三行\n\ntrailing \t  '

class Run:
    def __init__(self, mode):
        self.mode=mode; self.out=EVIDENCE/mode; self.out.mkdir(exist_ok=True)
        self.root=Path(tempfile.mkdtemp(prefix='bus-codexspikeb-',dir='/private/tmp')); self.root.chmod(0o700)
        for name in ['home','bin','fakebin','data','project','zsh','tmp','audit']: (self.root/name).mkdir(mode=0o700)
        for name in ['bus','bus-fake-agent']: (self.root/'bin'/name).symlink_to(BINARY)
        for name in ['claude','codex','cursor-agent']:
            fake=self.root/'fakebin'/name;fake.symlink_to(BINARY)
            shim=self.root/'bin'/name;shim.write_text('#!/bin/sh\np='+str(self.root/'audit')+'/$$\nmkdir -m 700 \"$p\"\nexport BUS_SPIKE_AUDIT_DIR=\"$p\"\nexec '+str(fake)+' \"$@\"\n');shim.chmod(0o700)
        for name in ['open','osascript','terminal-notifier','afplay','lark-cli']:
            p=self.root/'bin'/name; p.write_text('#!/bin/sh\nexit 0\n'); p.chmod(0o700)
        self.env={'HOME':str(self.root/'home'),'TMPDIR':str(self.root/'tmp'),'PATH':f'{self.root}/bin:/usr/bin:/bin','SHELL':'/bin/zsh','ZDOTDIR':str(self.root/'zsh'),'TERM':'xterm-256color','LANG':'en_US.UTF-8','USER':'spike','LOGNAME':'spike','BUS_DATA_DIR':str(self.root/'data'),'BUS_TUI_FAKE_AGENT':'1','BUS_SPIKE_AUDIT_DIR':str(self.root/'audit')}
        if mode.startswith('after'): self.env['BUS_SPIKE_FAST_CONFIRM']='1'

        for name in ['.zprofile','.zshrc']: (self.root/'zsh'/name).write_text('export PATH='+str(self.root/'bin')+':/usr/bin:/bin\n')
        self.rows=[]; self.term=None; self.stop=threading.Event(); self.agent_info={}
        self.save('setup',{'scratch':str(self.root),'mode':mode,'binary':str(BINARY),'version':subprocess.check_output([str(BINARY),'--version'],text=True).strip(),'binary_sha256':hashlib.sha256(BINARY.read_bytes()).hexdigest(),'payload_bytes':len(PAYLOAD.encode()),'draft':DRAFT})
    def save(self,name,value):
        (self.out/(name+'.json')).write_text(json.dumps(value,ensure_ascii=False,indent=2)+'\n');return value
    def control(self,*args,optional=False):
        if self.mode.endswith('-parallel'):
            if args[0]=='state':return self.raw_control('state',{},optional)
            if args[:2]==('agent','observe'):return self.raw_control('agent.observe',{'agent':str(args[2])},optional)
            if args[:2]==('message','status'):return self.raw_control('message.status',{'message':str(args[2])},optional)
            if args[0]=='send':
                def flag(name):return str(args[args.index(name)+1])
                return self.raw_control('message.send',{'room':flag('--room'),'to':[flag('--to')],'text':flag('--text'),'queue':'--queue' in args},optional)
        p=subprocess.run([str(BINARY),*map(str,args)],cwd=self.root/'project',env=self.env,text=True,capture_output=True,timeout=30)
        try: v=json.loads(p.stdout)
        except Exception:
            if optional:return None
            raise RuntimeError(f'{args}: {p.returncode}: {p.stdout} {p.stderr}')
        if not p.returncode and (v.get('ok') or args[0] in ['--paths','stop']): return v.get('result',v)
        if optional:return None
        raise RuntimeError(f'{args}: {v}')
    def raw_control(self,method,params,optional=False):
        try:
            with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as sock:
                sock.settimeout(30);sock.connect(str(self.root/'data'/('wait.sock' if method=='message.status' else 'control.sock')))
                sock.sendall((json.dumps({'id':'spike-'+secrets.token_hex(8),'method':method,'params':params})+'\n').encode())
                with sock.makefile('rb') as stream:value=json.loads(stream.readline())
            if value.get('ok'):return value['result']
            if method=='agent.observe' and any(message in (value.get('error') or {}).get('message','') for message in ['Resource temporarily unavailable','Terminal is redrawing']):
                with (self.out/'transient-read-errors.jsonl').open('a') as f:f.write(json.dumps(value)+'\n')
                return {}
            if optional:return None
            raise RuntimeError(value)
        except (OSError,ValueError):
            if optional:return None
            raise
    def native(self,method,params):
        with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as s:
            s.settimeout(30);s.connect(str(self.root/'data/herdr-config/sessions/bus/herdr.sock'))
            s.sendall((json.dumps({'id':secrets.token_hex(8),'method':method,'params':params})+'\n').encode())
            with s.makefile('rb') as f:v=json.loads(f.readline())
        if 'error' in v: raise RuntimeError(v)
        return v['result']
    def wait(self,desc,fn,seconds=35):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            v=fn()
            if v:return v
            time.sleep(.025)
        raise TimeoutError(desc)
    def audit(self):
        records=[]
        for p in (self.root/'audit').rglob('fake.jsonl'):
            records.extend(json.loads(l) for l in p.read_bytes().splitlines(keepends=True) if l.endswith(b'\n') and l.strip())
        return sorted(records,key=lambda r:r['wall_ns'])
    def state_agent(self,aid):
        return next(a for a in self.control('state')['agents'] if str(a['id'])==str(aid))
    def idle(self,aid):
        return self.wait('idle '+str(aid),lambda:(lambda a:a if a.get('status')=='idle' and not a.get('current_request') else None)(self.state_agent(aid)))
    def observe(self,aid):return self.control('agent','observe',aid)
    def send(self,aid,text,scenario,queue=False):
        t0=time.time_ns(); receipt=self.control('send','--room',self.room,'--to',aid,'--text',text,*(['--queue'] if queue else [])); t1=time.time_ns()
        row={'scenario':scenario,'agent':str(aid),'text':text,'send_call_ns':t0,'receipt_ns':t1,**receipt};self.rows.append(row)
        return row
    def complete(self,row,timeout=60,tolerate=False):
        try:
            status=self.wait('message '+str(row['message_id']),lambda:(lambda s:s if s['complete'] else None)(self.control('message','status',row['message_id'])),timeout)
        except TimeoutError:
            row['status']=self.control('message','status',row['message_id']);row['timed_out']=True
            row['observation']=self.observe(row['agent']);row['agent_after']=self.state_agent(row['agent']);self.save('samples',self.rows)
            if not tolerate:raise
            return row['status']
        row['status']=status;row['observation']=self.observe(row['agent']);self.save('samples',self.rows);return status
    def add(self,name,provider):
        project=self.root/'project'/name;project.mkdir()
        try:
            a=self.control('agent','add','--room',self.room,'--name',name,'--provider',provider,'--pwd',project,'--consent-hooks')
        except RuntimeError as error:
            if 'outcome requires inspection' not in str(error):raise
            self.save('uncertain-add-'+name,{'error':str(error)})
            owned=[a for a in self.control('state')['agents'] if a['name']==name]
            if len(owned)!=1:raise
            a={'agent_id':owned[0]['id']}
        aid=str(a['agent_id']); self.wait('ready '+name,lambda:'fake '+provider in self.observe(aid).get('text','').lower() or ('›' in self.observe(aid).get('text','') if provider=='codex' else False))
        self.agent_info[aid]=self.state_agent(aid); return aid
    def launch(self):
        self.term=terminal_module.Terminal([str(BINARY),'--dev'],self.root/'project',self.env)
        def pump():
            while not self.stop.is_set():self.term.pump(.1)
        self.thread=threading.Thread(target=pump,daemon=True);self.thread.start()
        state=self.wait('startup',lambda:self.control('state',optional=True));self.save('identity',state['build'])
        assert Path(state['build']['binary']).resolve()==BINARY.resolve(),state['build']
        self.room=str(self.control('room','create','spike-compare')['room_id'])
    def matrix(self):
        self.launch()
        claude=self.add('fixture-claude','claude'); codex=self.add('fixture-codex','codex')
        print(self.mode,'agents',claude,codex,flush=True)
        # Warmup is retained but excluded from scenario statistics.
        self.complete(self.send(codex,'warmup','warmup'));self.idle(codex)
        if self.mode=='smoke':
            self.complete(self.send(claude,'S1 smoke','S1'));self.idle(claude);return
        for i in range(10):
            self.complete(self.send(claude,f'S1 empty {i}','S1'));self.idle(claude)
        print(self.mode,'S1 done',flush=True)
        drafter=self.add('fixture-draft','claude')
        self.complete(self.send(drafter,'warmup','warmup'));self.idle(drafter)
        pane=self.agent_info[drafter]['runtime_identity']['pane_id']
        self.native('pane.send_input',{'pane_id':pane,'text':DRAFT})
        self.wait('developer draft rendered',lambda:any(r['event']=='draft' and r['data'].get('draft')==DRAFT for r in self.audit()))
        for i in range(10):
            row=self.send(drafter,f'S2 preserve {i}','S2')
            self.complete(row,timeout=15,tolerate=True)
            if row.get('timed_out'):
                self.complete(self.send(drafter,'S2 next delivery probe','S2-next'),timeout=3,tolerate=True)
                break
            self.wait('S2 request ended',lambda:not self.state_agent(drafter).get('current_request'))
        print(self.mode,'S2 done',flush=True)
        for i in range(5):
            lead=self.send(codex,f'fake:work 3 S3 lead {i}','setup')
            self.wait('working',lambda:self.state_agent(codex)['status']=='working')
            row=self.send(codex,PAYLOAD,'S3');self.complete(row);self.complete(lead);self.idle(codex)
        print(self.mode,'S3 done',flush=True)
        for i in range(5):
            lead=self.send(codex,f'fake:dialog 4 S4 setup {i}','setup')
            self.wait('dialog visible',lambda:'Do you want to proceed?' in self.observe(codex).get('text',''))
            row=self.send(codex,f'S4 deferred {i}','S4');self.complete(row);self.complete(lead);self.idle(codex)
        print(self.mode,'S4 done',flush=True)
        for i in range(30):
            self.complete(self.send(codex,f'S5 sequential {i}','S5'));self.idle(codex)
        peers=[codex]+[self.add(f'parallel-{i}','codex') for i in range(4)]
        for aid in peers[1:]:self.complete(self.send(aid,'warmup','warmup'));self.idle(aid)
        for i in range(10):
            with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
                rows=list(pool.map(lambda aid:self.send(aid,f'S5 parallel {i} agent {aid}','S5'),peers))
            for row in rows:self.complete(row)
            for aid in peers:self.idle(aid)
        print(self.mode,'S5 done',flush=True)
        self.save('agents',self.agent_info);self.save('samples',self.rows);self.save('fake-audit',self.audit())
    def tail_matrix(self):
        """Continue S4/S5 in a fresh fixture after archiving the S3 failure."""
        self.launch()
        codex=self.add('fixture-codex','codex')
        self.complete(self.send(codex,'warmup','warmup'));self.idle(codex)
        for i in range(5):
            lead=self.send(codex,f'fake:dialog 4 S4 setup {i}','setup')
            self.wait('dialog visible',lambda:'Do you want to proceed?' in self.observe(codex).get('text',''))
            row=self.send(codex,f'S4 deferred {i}','S4');self.complete(row);self.complete(lead);self.idle(codex)
        print(self.mode,'S4 done',flush=True)
        for i in range(30):
            self.complete(self.send(codex,f'S5 sequential {i}','S5'));self.idle(codex)
        peers=[codex]+[self.add(f'parallel-{i}','codex') for i in range(4)]
        for aid in peers[1:]:self.complete(self.send(aid,'warmup','warmup'));self.idle(aid)
        for i in range(10):
            with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
                rows=list(pool.map(lambda aid:self.send(aid,f'S5 parallel {i} agent {aid}','S5'),peers))
            for row in rows:self.complete(row)
            for aid in peers:self.idle(aid)
        print(self.mode,'S5 done',flush=True)
        self.save('agents',self.agent_info);self.save('samples',self.rows);self.save('fake-audit',self.audit())
    def parallel_matrix(self):
        """Same accepted boundary, without five unnecessary CLI process launches."""
        self.launch()
        peers=[self.add(f'parallel-{i}','codex') for i in range(5)]
        for aid in peers:self.complete(self.send(aid,'warmup','warmup'));self.idle(aid)
        for i in range(10):
            with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
                rows=list(pool.map(lambda aid:self.send(aid,f'S5 parallel {i} agent {aid}','S5'),peers))
            for row in rows:self.complete(row)
            for aid in peers:self.idle(aid)
        self.save('agents',self.agent_info);self.save('samples',self.rows);self.save('fake-audit',self.audit())
        print(self.mode,'S5 parallel done',flush=True)
    def close(self):
        stopped=self.control('stop',optional=True);self.save('stop',stopped)
        self.stop.set()
        if hasattr(self,'thread'):self.thread.join(timeout=2)
        if self.term:
            (self.out/'bus-terminal.ansi').write_bytes(self.term.output);self.term.close()
        for pattern in ['*.jsonl','herdr-*.log*']:
            for index,p in enumerate(sorted((self.root/'data').rglob(pattern))):
                if p.is_file():shutil.copy2(p,self.out/(f'log-{index}-'+p.name))
        self.save('fake-audit',self.audit());self.save('samples',self.rows)
        if stopped:shutil.rmtree(self.root);self.save('cleanup',{'removed':str(self.root)})
        else:self.save('cleanup',{'retained':str(self.root),'reason':'stop not confirmed'})

if __name__=='__main__':
    for mode in sys.argv[1:] or ['before','after']:
        run=Run(mode)
        try:run.parallel_matrix() if mode.endswith('-parallel') else (run.tail_matrix() if mode.endswith('-tail') else run.matrix())
        except BaseException as e:run.save('failure',{'error':repr(e)});raise
        finally:run.close()
