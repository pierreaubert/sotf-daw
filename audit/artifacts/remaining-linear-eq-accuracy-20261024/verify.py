"""Verify scoped evidence and current owned source fingerprints; no builds."""
from pathlib import Path
import hashlib,json,re
root=Path(__file__).resolve().parents[3]
base=Path(__file__).resolve().parent
summary=json.loads((base/'summary.json').read_text())
for name in summary['required_success_records']:
    result=json.loads((base/name).read_text())
    assert result['exit_code']==0,name
    if 'locks_unchanged' in result: assert result['locks_unchanged'],name
    if 'lock_unchanged' in result: assert result['lock_unchanged'],name
core=json.loads((base/'core-final-confirmed.json').read_text())
counts=re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored',core['stdout'])
assert tuple(sum(int(row[i]) for row in counts) for i in range(3))==(152,0,2)
assert '15 passed; 0 failed' in json.loads((base/'ffi-legacy-coherent.json').read_text())['stdout']
for name,info in json.loads((base/'source-manifest.json').read_text())['paths'].items():
    assert hashlib.sha256((root/name).read_bytes()).hexdigest()==info['sha256'],name
partner=json.loads((base/'delegated-host-pdc.json').read_text())
assert partner['test']['passed']==1 and partner['strict_clippy_exit_code']==0
assert hashlib.sha256((root/partner['source']).read_bytes()).hexdigest()==partner['source_sha256']
assert all(row['source_unchanged_from_HEAD'] for row in json.loads((base/'ffi-release-lint-baseline-source-trace.json').read_text())['functions'])
print('PASS: owned source fingerprints;152 core /15 coherent FFI; debug, heap, factory, native and strict records; delegated observable host PDC. Retained failures are excluded from success.')
