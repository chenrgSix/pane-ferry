#!/usr/bin/env python3
"""Recovery semantics without a desktop, account, or debug server."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

root = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('page_scale', root/'apps/linux-agent/src/electron_page_scale.py')
helper = importlib.util.module_from_spec(spec); spec.loader.exec_module(helper)


class TargetTests(unittest.TestCase):
    def test_multiple_matching_pages_are_rejected_before_mutation(self):
        class Response:
            def __enter__(self): return self
            def __exit__(self, *_): pass
            def read(self, _): return json.dumps([dict(type='page', url='app://-/index.html', id='A'), dict(type='page', url='app://-/index.html', id='B')]).encode()
        class Opener:
            def open(self, *_args, **_kw): return Response()
        class CDP:
            def __init__(self, *_): pass
            def js(self, _): return dict(bounds=[1, 2, 900, 600], dpr=1, top=True, url='app://-/index.html')
            def close(self): pass
        with patch.object(helper.urllib.request, 'build_opener', return_value=Opener()), patch.object(helper, 'CDP', CDP):
            with self.assertRaisesRegex(ValueError, 'page_scale_target_ambiguous'):
                helper.select_target(12345, [1, 2, 900, 600])

    def test_helper_restores_on_eof_and_systemd_termination(self):
        code = """
import importlib.util, pathlib, sys
spec=importlib.util.spec_from_file_location('helper',sys.argv[1])
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class Lease:
    def __init__(self,_): self.size=[800,600]
    def tick(self): pass
    def restore(self): pathlib.Path(sys.argv[2]).write_text('restored');return True
m.Lease=Lease;m.serve()
"""
        with tempfile.TemporaryDirectory() as directory:
            for stop in ('eof', 'sigterm'):
                marker = Path(directory)/stop
                child = subprocess.Popen([sys.executable, '-c', code, str(root/'apps/linux-agent/src/electron_page_scale.py'), str(marker)],
                                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                try:
                    child.stdin.write(b'{"op":"start"}\n');child.stdin.flush()
                    self.assertEqual(json.loads(child.stdout.readline()), {'size':[800,600]})
                    if stop == 'eof': child.stdin.close()
                    else: child.terminate()
                    self.assertEqual(child.wait(timeout=5), 0)
                    self.assertEqual(marker.read_text(), 'restored')
                finally:
                    if child.poll() is None: child.kill();child.wait()
                    for pipe in (child.stdin,child.stdout,child.stderr): pipe.close()

    def test_dom_restore_expiry_renewal_and_frame_scope(self):
        script = '''const vm=require('node:vm'), assert=require('node:assert/strict');
const install=INSTALL;
function page(value='', priority='') {
 const saved={value,priority}; let now=0, timer;
 const sheet={isConnected:false,remove(){this.isConnected=false}};
 const style={getPropertyValue:()=>saved.value,getPropertyPriority:()=>saved.priority,
   setProperty:(_n,v,p)=>Object.assign(saved,{value:v,priority:p}),removeProperty:()=>Object.assign(saved,{value:'',priority:''})};
 const p={document:{documentElement:{style},querySelector:()=>({}),querySelectorAll:()=>[],createElement:()=>sheet,head:{append:s=>{s.isConnected=true}}},location:{origin:'app://-',pathname:'/index.html'},
 performance:{now:()=>now},getComputedStyle:()=>({getPropertyValue:()=>saved.value||'1'}),setInterval:f=>(timer=f,1),clearInterval:()=>{timer=null}};
 p.top=p.self=p;vm.createContext(p);
 return {p,saved,sheet,run:()=>vm.runInContext(install,p),expire:()=>{now+=5000;timer?.()}};
}
for(const original of [['',''],['0.8','important']]) {
 const t=page(...original); assert.equal(t.run(),true);
 assert.equal(Number(t.saved.value),Number(original[0]||1)*2);
 t.run(); assert.equal(Number(t.saved.value),Number(original[0]||1)*2,'renewal must not compound zoom');
 t.expire();assert.equal(t.sheet.isConnected,false,'expiry removes browser layout adaptation');assert.deepEqual([t.saved.value,t.saved.priority],original,'expiry restores absence/priority exactly');
 t.run();t.p.testLease.restore();assert.deepEqual([t.saved.value,t.saved.priority],original);
 t.run();t.saved.value='3';assert.equal(t.run(),false,'external change must end the lease');t.p.testLease.restore();
}
let t=page();t.p.document.querySelector=()=>null;assert.equal(t.run(),false,'unknown layout must not be changed');
t=page();t.p.document.querySelectorAll=()=>[{getBoundingClientRect:()=>({width:300,height:200}),matches:()=>false}];assert.equal(t.run(),false,'unknown visible embedded views are rejected');
t=page();t.p.top={};assert.equal(t.run(),false);assert.equal(t.saved.value,'');
t=page();t.p.location.origin='https://example.test';assert.equal(t.run(),false);assert.equal(t.saved.value,'');
console.log('PASS: CSS renewal, expiry, exact restore, top frame and application origin scope');
'''.replace('INSTALL', json.dumps(helper.installation('testLease')))
        subprocess.run(['node', '-e', script], check=True)


if __name__ == '__main__': unittest.main()
