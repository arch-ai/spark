#!/usr/bin/env python3
"""Exercise live container RAM with a fake Docker CLI and blocking FIFO streams."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

sys.dont_write_bytecode = True
from qa_docker_tui import Screen

DOCKER = r'''#!/usr/bin/python3
import json,os,pathlib,signal,sys
r=pathlib.Path(os.environ['SPARK_QA_DIR']);a=sys.argv[1:]
with (r/'commands').open('a') as f:print(json.dumps(a),file=f)
ids={'alpha':'a'*64,'beta':'b'*64,'gamma':'c'*64}
if a[0]=='stats':
    assert a==['stats','--no-trunc','--format','{{json .}}'],a
    for name,usage,pct in [('alpha','512MiB / 2GiB','25.00%'),('beta','1.5GiB / 2GiB','75.00%'),('gamma','2GiB / 4GiB','50.00%')]:
        print(json.dumps({'ID':ids[name],'MemUsage':usage,'MemPerc':pct}),flush=True)
    with (r/'stats.pipe').open() as stream:
        for line in stream:
            if line.strip()=='EXIT':
                print('fixture stats disconnected',file=sys.stderr,flush=True);sys.exit(2)
            print(line.strip(),flush=True)
elif a[0]=='ps':
    if '-a' in a:
        for name in ids:
            status='Exited (0) 1 minute ago' if name=='gamma' or (name=='beta' and (r/'stopped').exists()) else 'Up 2 hours (healthy)'
            print(ids[name]+'|'+name+'|demo:v1||'+status+'|com.docker.compose.project=memory-demo,com.docker.compose.project.working_dir='+str(r))
elif a[:2]==['system','df']:
    print('Images|1|1|2MB|0B (0%)\nContainers|3|2|0B|0B (0%)\nLocal Volumes|0|0|0B|0B (0%)\nBuild Cache|0|0|0B|0B (0%)')
elif a[0]=='events':signal.pause()
else:
    print('Unexpected fixture command: '+repr(a),file=sys.stderr);sys.exit(3)
'''


def main():
    binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/spark').resolve())
    with tempfile.TemporaryDirectory(prefix='spark-memory-') as directory:
        root = Path(directory)
        (root/'docker').write_text(DOCKER)
        (root/'docker').chmod(0o755)
        (root/'pm2').write_text('#!/bin/sh\necho "[]"\n')
        (root/'pm2').chmod(0o755)
        (root/'package.json').write_text('{"name":"memory-demo"}')
        config = root/'config'
        config.mkdir()
        (config/'workspace.json').write_text(json.dumps({'version': 1, 'view': 'docker'}))
        os.mkfifo(root/'stats.pipe')
        writer = os.open(root/'stats.pipe', os.O_RDWR | os.O_NONBLOCK)
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        width, height = 160, 36
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', height, width, 0, 0))
        env = dict(os.environ, PATH=f'{root}:'+os.environ['PATH'], TERM='xterm-256color', SPARK_QA_DIR=directory, SPARK_CONFIG_DIR=str(config))
        proc = subprocess.Popen([binary], stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True)
        screen = Screen(width, height)

        def expect(label, predicate=None, timeout=10):
            predicate = predicate or (lambda text: label in text)
            deadline = time.monotonic() + timeout
            while not predicate(screen.text()):
                ready, _, _ = select.select([master], [], [], max(0, deadline-time.monotonic()))
                if not ready:
                    raise AssertionError(f'Expected {label}:\n{screen.text()}')
                screen.feed(os.read(master, 65536))

        def send(value):
            os.write(master, value if isinstance(value, bytes) else value.encode())

        def click(label, marker=None):
            expect(label, lambda text: any(label in row and (marker is None or marker in row) for row in text.splitlines()))
            for y, row in enumerate(screen.text().splitlines()):
                if label in row and (marker is None or marker in row):
                    x = row.index(label) + len(label)//2
                    send(f'\x1b[<0;{x+1};{y+1}M')
                    return

        def emit(name, usage, percent):
            os.write(writer, (json.dumps({'ID': {'alpha': 'a'*64, 'beta': 'b'*64}[name], 'MemUsage': usage, 'MemPerc': percent})+'\n').encode())

        def starts():
            return sum(json.loads(line)[0] == 'stats' for line in (root/'commands').read_text().splitlines())

        try:
            expect('DOCKER')
            expect('512M')
            expect('1.5G')
            assert '2.0G' not in screen.text(), 'A stopped container must not display a queued measurement'
            click('RAM', 'NAME')
            expect('RAM ▼')
            expect('numeric memory order', lambda text: text.index('beta') < text.index('alpha'))
            click('beta', '1.5G')
            send(b'\x1bOQ')
            expect('Docker · beta')
            send(b'\x1b')
            expect('selection acknowledged', lambda text: 'Docker · beta' not in text)
            emit('alpha', '3GiB / 4GiB', '75.00%')
            expect('3.0G')
            expect('resorted memory', lambda text: text.index('alpha') < text.index('beta'))
            send(b'\x1bOQ')
            expect('Docker · beta')
            expect('1.50 GiB / 2.00 GiB')
            expect('75.00% of limit')
            emit('beta', '256MiB / 1GiB', '25.00%')
            expect('256.00 MiB / 1.00 GiB')
            expect('25.00% of limit')
            send(b'\x1b')
            expect('table restored', lambda text: 'Docker · beta' not in text and '256M' in text)
            os.write(writer, b'EXIT\n')
            expect('fixture stats disconnected')
            expect('256M*')
            assert starts() == 1, 'Stream failure must not cause a recurring restart loop'
            send(b'\x1b[15~')
            expect('stats reconnected', lambda text: '1.5G' in text and 'fixture stats disconnected' not in text and '1.5G*' not in text)
            assert starts() == 2
            # Refresh container state while a valid memory stream is still emitting beta.
            (root/'stopped').touch()
            send(b'\x1b[15~')
            expect('stopped row', lambda text: any('beta' in row and 'Exited' in row and '1.5G' not in row for row in text.splitlines()))
            click('beta', 'Exited')
            send(b'\x1bOQ')
            expect('Memory: not running')
            send(b'\x1b')
            expect('back to containers', lambda text: 'Docker · beta' not in text)
            send('/alpha\r')
            expect('Filter: alpha')
            width, height = 30, 10
            screen = Screen(width, height)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', height, width, 0, 0))
            os.kill(proc.pid, signal.SIGWINCH)
            expect('RAM ▼')
            expect('512M')
            send('q')
            proc.wait(timeout=3)
            assert proc.returncode == 0
            assert termios.tcgetattr(slave) == original
            saved = json.loads((config/'workspace.json').read_text())
            assert any(s['target'] == 'Docker' and s['field'] == 'Memory' for s in saved['sorts'])
            print('PASS: streamed container RAM, numeric header sort, stable selection during updates, live usage/limit/percentage details, stopped-container handling, failure/cache/reconnect, compact layout, persisted sorting, clean exit. Docker commands used fixtures only.')
        finally:
            os.close(writer)
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=3)
            os.close(master)
            os.close(slave)


if __name__ == '__main__':
    main()
