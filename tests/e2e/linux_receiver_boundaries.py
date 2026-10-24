#!/usr/bin/env python3
"""Exercise production parsing without loading a decoder for hostile streams."""
import ast, sys, re
from pathlib import Path
root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / 'apps/linux-client'))
import wire, clipboard
source = ast.parse((root/'apps/linux-client/windowhost.py').read_text())
namespace = {}
exec(compile(ast.Module(body=[n for n in source.body if isinstance(n,ast.FunctionDef) and n.name in ('bounds','check_sps','parameter_sets')], type_ignores=[]), '<production>', 'exec'), namespace)
def ue(n):
    bits = bin(n+1)[2:]; return '0'*(len(bits)-1)+bits
def sps(width, height, crop_right=0, refs=1):
    bits = f'{66:08b}'+'0'*8+f'{40:08b}'+ue(0)+ue(0)+ue(0)+ue(0)+ue(refs)+'0'+ue(width//16-1)+ue(height//16-1)+'11'
    bits += ('1'+ue(0)+ue(crop_right)+ue(0)+ue(0)) if crop_right else '0'
    bits+='1'; bits+='0'*((-len(bits))%8)
    raw=bytes(int(bits[i:i+8],2) for i in range(0,len(bits),8))
    return b'\0\0\1\x67'+raw.replace(b'\0\0',b'\0\0\3')
check = namespace['check_sps']
assert check(sps(640,480),640,480)
assert not check(b'\0\0\1\x65abc',640,480)
for data,w,h in [(sps(65536,16,(65536-16)//2),16,16),(sps(4096,1920,refs=16),4096,1920),(sps(640,480),320,240)]:
    try: check(data,w,h); raise AssertionError('unsafe SPS accepted')
    except ValueError: pass
class Board:
    def connect(self, signal, callback): self.changed=callback
board=Board();clipboard.watch(board)
token=clipboard.begin_copy();board.changed(board,None)
assert token != clipboard.generation, 'external owner change must invalidate a pending copy'
text=(root/'crates/transport/src/lib.rs').read_text().split('pub fn retry_same_request')[1].split('_ => true')[0]
assert wire.TERMINAL_LAUNCH_ERRORS == set(re.findall(r'"([a-z_]+)"',text))
print('PASS coded size, DPB, absent SPS, external clipboard owner, shared terminal-error policy')
