import os,sys,json,time,subprocess,hashlib,pathlib
root=pathlib.Path('/Volumes/home_tmp/src/all_of_sotf/sotf-daw')
out=root/'audit/artifacts/remaining-linear-eq-accuracy-20261024'
fixture=pathlib.Path('/Volumes/home_tmp/tmp/plugin-lock-resolution-8zjflj2u')
locks=[root/'Cargo.lock',fixture/'Cargo.lock']
def hashes(): return {str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in locks}
env=dict(os.environ,CARGO_BUILD_JOBS='2',TMPDIR='/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-nupc-temp')
target=['--target-dir','/Volumes/home_tmp/tmp/sotf-daw-audit-20261018-target']
mode=sys.argv[1]
if mode=='nih':
 jobs=[('nih-native-lifecycle-final',root,['test','-p','plugins-nih','--lib','--no-default-features','--features','linear-phase-eq','linear_phase_eq_native_mode_only','--release','--offline','--locked',*target,'--','--nocapture']),('nih-legacy-clippy-final',root,['clippy','-p','plugins-nih','--no-default-features','--features','linear-phase-eq','--all-targets','--release','--offline','--locked',*target,'--','-D','warnings'])]
else:
 jobs=[('ffi-legacy-coherent-confirmed',fixture,['test','-p','plugins-ffi','--lib','linear_phase','--release','--offline','--locked','--features','sotf-plugins/qa',*target,'--','--nocapture']),('ffi-legacy-clippy-debug-coherent',fixture,['clippy','-p','plugins-ffi','--all-targets','--offline','--locked','--features','sotf-plugins/qa',*target,'--','-D','warnings'])]
if mode=='ffi-clippy': jobs=jobs[1:]
for name,cwd,args in jobs:
 before=hashes();start=time.monotonic();cmd=['rtk','proxy','cargo',*args]
 r=subprocess.run(cmd,cwd=cwd,env=env,text=True,capture_output=True)
 after=hashes();record=dict(command=cmd,cwd=str(cwd),exit_code=r.returncode,elapsed_seconds=time.monotonic()-start,stdout=r.stdout,stderr=r.stderr,locks_sha256_before=before,locks_sha256_after=after,locks_unchanged=before==after)
 if cwd==fixture:record['scope']='Known coherent isolated GPUI dependency baseline, source symlinks; ordinary main FFI dependency graph remains separately blocked.'
 (out/(name+'.json')).write_text(json.dumps(record,indent=2)+'\n')
 print(name,r.returncode,r.stdout[-2500:],r.stderr[-3500:],flush=True)
 if r.returncode or before!=after:break
