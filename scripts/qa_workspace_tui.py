#!/usr/bin/env python3
"""Projects/inspector PTY acceptance checks. All external services/scripts are fixtures."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
sys.dont_write_bytecode = True
from qa_docker_tui import Screen

DOCKER = '''#!/usr/bin/python3
import json,os,pathlib,signal,sys
root=pathlib.Path(os.environ['SPARK_QA_DIR']);a=sys.argv[1:];cid='0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
with (root/'commands').open('a') as log: print(json.dumps(a),file=log)
volumes=['data','cache','keep']; alive=not (root/'container-deleted').exists()
labels='com.docker.compose.project=qa-workspace,com.docker.compose.project.working_dir='+str(root)
if a[0]=='stats':
    signal.pause()
elif a[0]=='events':
    with (root/'events').open() as stream:
        for line in stream: print(line.strip(),flush=True)
elif a[0]=='logs':
    print('READY api stream',flush=True)
    with (root/'logs').open() as stream:
        for line in stream: print(line.strip(),flush=True)
elif a[:2]==['volume','ls']:
    for v in volumes:
        if not (root/('deleted-'+v)).exists(): print(v)
elif a[:3]==['system','df','-v']:
    print('Local Volumes space usage:\\nNAME LINKS SIZE')
    for v,size in [('data','950 GB'),('cache','1 GB'),('keep','5 GB')]:
        if not (root/('deleted-'+v)).exists():print(v+' '+('1' if alive else '0')+' '+size)
    print('Build cache space usage:')
elif a[:2]==['system','df']:
    print('Images|1|1|2MB|0B (0%)\\nContainers|1|1|0B|0B (0%)\\nLocal Volumes|3|3|956GB|0B (0%)\\nBuild Cache|0|0|0B|0B (0%)')
elif a[0]=='ps':
    if '--filter' in a:
        if alive:print('changed-owner' if (root/'changed').exists() else cid)
    elif '-q' in a:
        if alive:print(cid)
    elif alive:
        prefix=cid+'|api|demo:v1|0.0.0.0:8080->80/tcp|'
        print(prefix+('Up 2 hours|'+labels if '-a' in a else labels))
elif a[:3]==['inspect','--type','container']:
    print(json.dumps({'Id':cid,'Name':'/api','Status':'running','FinishedAt':'0001-01-01T00:00:00Z','Mounts':[{'Type':'volume','Name':v} for v in volumes],'Image':'demo:v1','Project':'qa-workspace','WorkingDir':str(root)}))
elif a[0]=='inspect':
    assert a==['inspect','--',cid],a
    print(json.dumps([{'Id':cid,'Name':'/api','Config':{'Image':'demo:v1','Labels':{'com.docker.compose.project':'qa-workspace'}},'State':{'Status':'running'},'Mounts':[{'Type':'volume','Name':v} for v in volumes]}]))
elif a[0]=='rm':
    assert a==['rm','-f','--',cid],a
    (root/'container-deleted').touch();print(cid)
elif a[:2]==['volume','rm']:
    if a[-1]=='cache' and (root/'slow-remove').exists():
        with (root/'remove-gate').open() as gate:gate.readline()
    assert a==['volume','rm','--',a[-1]] and a[-1] in ['data','cache'],a
    assert (root/'container-deleted').exists()
    (root/('deleted-'+a[-1])).touch();print(a[-1])
else:
    print('Unexpected fake Docker command '+repr(a),file=sys.stderr);sys.exit(2)
'''
JOURNAL = '''#!/usr/bin/python3
import os,pathlib,sys
root=pathlib.Path(os.environ['SPARK_QA_DIR']);a=sys.argv[1:]
assert '--boot' in a and '--since' in a and a[a.index('--since')+1].startswith('@'),a
print('READY native journal stream',flush=True)
with (root/'nativelogs').open() as stream:
    for line in stream:print(line.strip(),flush=True)
'''
PM2 = '''#!/usr/bin/python3
import json,os,pathlib,sys
root=pathlib.Path(os.environ['SPARK_QA_DIR']);a=sys.argv[1:]
if a[0]=='jlist':
    print(json.dumps([{'pm_id':7,'name':'qa-worker','pid':int(os.environ['SPARK_NATIVE_PID']),'monit':{'cpu':1.5,'memory':47185920},'pm2_env':{'status':'online','exec_mode':'fork_mode','pm_exec_path':str(root/'app.js'),'pm_cwd':str(root)}}]))
elif a[0]=='logs':
    print('READY pm2 stream',flush=True)
    with (root/'pm2logs').open() as stream:
        for line in stream:print(line.strip(),flush=True)
else:sys.exit(2)
'''


def main():
    binary=str(Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/spark').resolve())
    with tempfile.TemporaryDirectory(prefix='spark-workspace-') as directory:
        root=Path(directory)
        for name,source in [('docker',DOCKER),('pm2',PM2),('journalctl',JOURNAL)]:
            (root/name).write_text(source);(root/name).chmod(0o755)
        for name in ['events','logs','pm2logs','nativelogs','remove-gate']:os.mkfifo(root/name)
        for name in ['start-dev','stop-dev','start-prod','stop-prod']:
            script=root/name
            script.write_text('#!/bin/sh\n'+f"printf '{name}\\n' >> order\n"+('if [ -f fail-stop ]; then echo STOP_FAILED >&2; exit 9; fi\n' if name.startswith('stop') else '')+f"printf '{name} done\\n'\n")
            script.chmod(0o755)
        (root/'package.json').write_text('{"name":"qa-workspace"}')
        (root/'app.js').write_text('')
        (root/'node').write_bytes(Path(shutil.which('cat')).read_bytes());(root/'node').chmod(0o755)
        native=subprocess.Popen([str(root/'node'),str(root/'app.js'),'-'],cwd=root,stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        (root/'server.py').write_text('import socket,sys\ns=socket.socket();s.bind(("127.0.0.1",0));s.listen();print(s.getsockname()[1],flush=True);sys.stdin.read()\n')
        server=subprocess.Popen([sys.executable,'-u',str(root/'server.py')],cwd=root,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        native_port=server.stdout.readline().decode().strip()
        if not native_port:
            error=server.stderr.read().decode();server.wait(timeout=2);native.terminate();native.wait(timeout=2);raise RuntimeError('Owned localhost fixture could not bind: '+error)
        config=root/'config';config.mkdir();config_file=config/'workspace.json'
        config_file.write_text(json.dumps({'version':1,'view':'projects','projects':[{'name':'qa-workspace','path':str(root),'start_dev':'./start-dev','stop_dev':'./stop-dev','start_prod':'./start-prod','stop_prod':'./stop-prod'}]}))
        master,slave=pty.openpty();original=termios.tcgetattr(slave);width,height=160,36
        fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',height,width,0,0))
        env=dict(os.environ,PATH=f'{root}:'+os.environ['PATH'],TERM='xterm-256color',SPARK_QA_DIR=directory,SPARK_CONFIG_DIR=str(config),SPARK_NATIVE_PID=str(native.pid))
        proc=subprocess.Popen([binary],stdin=slave,stdout=slave,stderr=slave,env=env,start_new_session=True);screen=Screen(width,height);writers=[]

        def expect(label,timeout=10,predicate=None):
            predicate=predicate or (lambda text:label in text)
            if predicate(screen.text()):return
            deadline=time.monotonic()+timeout
            while True:
                ready,_,_=select.select([master],[],[],max(0,deadline-time.monotonic()))
                if not ready:raise AssertionError(f'Expected {label}:\n{screen.text()}')
                try:chunk=os.read(master,65536)
                except OSError:raise AssertionError(f'Spark exited while expecting {label}:\n{screen.text()}')
                screen.feed(chunk)
                if predicate(screen.text()):return

        def send(value):os.write(master,value if isinstance(value,bytes) else value.encode())
        def click(label,marker=None):
            expect(label,predicate=lambda text:any(label in row and (marker is None or marker in row) for row in text.splitlines()))
            for y,row in enumerate(screen.text().splitlines()):
                if label in row and (marker is None or marker in row):
                    x=row.index(label)+len(label)//2;send(f'\x1b[<0;{x+1};{y+1}M');return

        try:
            expect('PROJECTS');expect('qa-worker',predicate=lambda text:'qa-workspace' in text and 'Discovering' not in text)
            send('/qa-workspace\r');expect('Filter: qa-workspace');send('f');expect('* qa-workspace')
            click('CPU','RAM');expect('CPU ▼');send('W');expect('Saved filters');send('n');expect('Save current filter');send('QA\r');expect('Named filter saved')
            send('C');expect('Project scripts');send(b'\x13');expect('Project configuration saved')
            send('\r');expect('Project · qa-workspace');expect('api',predicate=lambda text:'Docker ' in text and 'api' in text)
            send('\r');expect('Docker · api');send('l');expect('READY api stream')
            logs=os.open(root/'logs',os.O_RDWR|os.O_NONBLOCK);writers.append(logs);os.write(logs,b'ERROR request failed\n');expect('ERROR request failed')
            send('/ERROR\r');expect('ERROR request failed');send('p');expect('Paused');os.write(logs,b'ERROR while paused\n');send('y')
            expect('clipboard request',predicate=lambda text:bool(screen.clipboards));assert 'ERROR while paused' not in screen.clipboards[-1]
            send('p');expect('ERROR while paused');send('e');expect('Events')
            events=os.open(root/'events',os.O_RDWR|os.O_NONBLOCK);writers.append(events)
            os.write(events,(json.dumps({'Type':'container','Action':'die','Actor':{'ID':'0123456789abcdef'*4,'Attributes':{'name':'api','exitCode':'137','com.docker.compose.project':'qa-workspace'}},'time':int(time.time())})+'\n').encode());expect('api · die · exit 137')
            send(b'\x7f');expect('Project · qa-workspace');click('qa-worker','PM2');send('\r');expect('PM2 · qa-worker');send('l');expect('READY pm2 stream');send(b'\x7f');expect('Project · qa-workspace');send('t');expect('samples · latest')
            send('D');expect('dev scripts completed successfully');assert (root/'order').read_text().splitlines()==['stop-prod','start-dev']
            send('P');expect('prod scripts completed successfully');assert (root/'order').read_text().splitlines()==['stop-prod','start-dev','stop-dev','start-prod']
            (root/'fail-stop').touch();send('D');expect('start script was not run');assert (root/'order').read_text().splitlines()[-1]=='stop-prod';(root/'fail-stop').unlink()
            (root/'start-dev').write_text('#!/bin/sh\necho FOREGROUND_READY\nexec sleep 30\n');send('D');expect('FOREGROUND_READY');send('D');expect('already running for this project');send(b'\x1b');expect('Project table retained',predicate=lambda text:'Project · qa-workspace' not in text);send('\r');expect('Project · qa-workspace');click('Run','Details');expect('FOREGROUND_READY');send('X');expect('Script failed');
            send('v');expect('950 GB');send(' ');send(b'\x1b[B\x1b[B');send(' ');send(b'\x1b[3~');expect('Delete 2 volumes and 1 containers?');expect('Other volumes retained: keep');send(b'\x1b');expect('Storage',predicate=lambda text:'Delete 2 volumes' not in text)
            assert not (root/'container-deleted').exists()
            send(b'\x1b[3~');expect('Delete 2 volumes and 1 containers?');(root/'changed').touch();send('y');expect('Volume references changed since review');assert not (root/'container-deleted').exists();send(b'\x1b');expect('Storage after failed review',predicate=lambda text:'Volume references changed' not in text and '950 GB' in text);(root/'changed').unlink()
            gate=os.open(root/'remove-gate',os.O_RDWR|os.O_NONBLOCK);writers.append(gate)
            send(b'\x1b[3~');expect('Delete 2 volumes and 1 containers?');(root/'slow-remove').touch();send('y');expect('Removing the reviewed containers');send(b'\x1b');expect('cleanup in background',predicate=lambda text:'Volume cleanup running' in text);send(b'\x1b[17~');expect('Removing the reviewed containers');send(b'\x1b');expect('cleanup in background',predicate=lambda text:'Volume cleanup running' in text);os.write(gate,b'continue\n');send(b'\x1b[17~');expect('Removed volume data');expect('Removed volume cache');(root/'slow-remove').unlink();assert (root/'container-deleted').exists();assert not (root/'deleted-keep').exists()
            mutations=[a for a in map(json.loads,(root/'commands').read_text().splitlines()) if a[0]=='rm'];assert len(mutations)==1 and '-v' not in mutations[0]
            send(b'\x1b');expect('Storage after cleanup',predicate=lambda text:'Removed volume data' not in text);send(b'\x1b');expect('Projects after inspector',predicate=lambda text:'Project · qa-workspace' not in text);send('2');expect('PORTS');send('/8080\r');expect('Filter: 8080')
            # Docker fixture restores the owner solely for navigation verification.
            (root/'container-deleted').unlink();send(b'\x1b[15~');expect('docker:api');send('\r');expect('DOCKER');expect('Selected resource owner');send(b'\x1bOQ');expect('Docker · api')
            # Recompose to one pane, then return to the retained table selection.
            width,height=70,20;screen=Screen(width,height);fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',height,width,0,0));os.kill(proc.pid,signal.SIGWINCH);expect('Docker · api');send(b'\x1b');expect('DOCKER')
            send('2');expect('PORTS');send('x/'+native_port+'\r');expect('Filter: '+native_port);click(native_port,'python');send('\r');expect('PROCESS VIEW');expect('Selected resource owner');send(b'\x1bOQ');expect('Process ·');send('l');expect('READY native journal stream');server.terminate();server.wait(timeout=2);expect('Stream ended');send(b'\x1b');expect('PROCESS VIEW');
            send('5');expect('PROJECTS');send('W');expect('Saved filters');send('\r');expect('Filter: qa-workspace');send('q');proc.wait(timeout=3);assert proc.returncode==0;assert termios.tcgetattr(slave)==original
            saved=json.loads(config_file.read_text());assert saved['filters']['projects']=='qa-workspace';assert saved['favorites']==['path:'+str(root)];assert saved['saved_filters'][0]['name']=='QA';assert any(s['target']=='Projects' and s['field']=='Cpu' for s in saved['sorts'])
            # Relaunch from the same persisted workspace.
            proc=subprocess.Popen([binary],stdin=slave,stdout=slave,stderr=slave,env=env,start_new_session=True);screen=Screen(width,height);expect('PROJECTS');expect('Filter: qa-workspace');expect('* qa-workspace');send('q');proc.wait(timeout=3);assert proc.returncode==0
            print('PASS: project relationships, clickable project sort, persisted filters/favorites/scripts, shared inspector and responsive panes, stream search/pause/follow/copy, Docker event cause, resource trends, dev/prod ordering and failed-stop guard, exact batch cleanup preview/cancel/reference-change guard, retained unrelated volumes, port-owner navigation, clean relaunch/exit and terminal restoration. External commands and scripts used fixtures only.')
        finally:
            for writer in writers:os.close(writer)
            native.terminate();native.wait(timeout=2)
            if server.returncode is None:server.terminate();server.wait(timeout=2)
            if proc.poll() is None:os.killpg(proc.pid,signal.SIGKILL);proc.wait(timeout=2)
            os.close(master);os.close(slave)


if __name__=='__main__':main()
