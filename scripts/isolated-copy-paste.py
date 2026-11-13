#!/usr/bin/python3
"""Run native clipboard tests on an authenticated nested X11 display, never the user's clipboard."""
import os
import secrets
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

def stop(process):
    if process and process.poll() is None:
        process.terminate()
        try: process.wait(timeout=5)
        except subprocess.TimeoutExpired: process.kill(); process.wait()

def main():
    mode = next((flag for flag in ['--collections-only','--pinned-only','--manual-only','--paste-performance','--wake-only','--startup-performance'] if flag in sys.argv), None)
    if '--inside' in sys.argv:
        wm = subprocess.Popen(['muffin', '--sm-disable'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            time.sleep(0.8)
            test_args = [mode] if mode else ['--clipboard-fixtures','--screenshots','docs/previews/copy-paste']
            if mode in ['--collections-only','--pinned-only']:
                test_args += ['--screenshots', 'docs/previews/collections' if mode=='--collections-only' else 'docs/previews/pinned']
            env = os.environ.copy()
            env['UCLIP_FIXTURE_WM_PID'] = str(wm.pid)
            return subprocess.call(['/usr/bin/python3', 'scripts/native-smoke.py',*test_args], env=env)
        finally: stop(wm)
    with tempfile.TemporaryDirectory(prefix='uclip-isolated-display-') as directory:
        display = next(f':{n}' for n in range(97, 120) if not Path(f'/tmp/.X{n}-lock').exists() and not Path(f'/tmp/.X11-unix/X{n}').exists())
        auth = str(Path(directory, 'Xauthority'))
        Path(auth).touch()
        subprocess.run(['xauth', '-f', auth, 'add', display, '.', secrets.token_hex(16)], check=True)
        with open(Path(directory, 'display.log'), 'w+') as log:
            server = subprocess.Popen(['Xephyr', display, '-screen', '1280x900', '-nolisten', 'tcp', '-noreset', '-auth', auth], stdout=log, stderr=log)
            try:
                env = os.environ.copy()
                env.update(DISPLAY=display, XAUTHORITY=auth, XDG_SESSION_TYPE='x11', UCLIP_FIXTURE_DISPLAY='1', GDK_SCALE='1', GDK_DPI_SCALE='1', NO_AT_BRIDGE='1')
                if '--playwright-module' in sys.argv:
                    env['UCLIP_PLAYWRIGHT_MODULE'] = sys.argv[sys.argv.index('--playwright-module') + 1]
                env.setdefault('PLAYWRIGHT_BROWSERS_PATH', str(Path(os.environ.get('XDG_CACHE_HOME', str(Path.home() / '.cache')), 'ms-playwright')))
                for name, suffix in [('XDG_DATA_HOME', 'data'), ('XDG_CONFIG_HOME', 'config'), ('XDG_CACHE_HOME', 'cache')]:
                    env[name] = str(Path(directory, suffix))
                env.pop('WAYLAND_DISPLAY', None)
                env.pop('DBUS_SESSION_BUS_ADDRESS', None)
                for _ in range(50):
                    if subprocess.run(['xdpyinfo'], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0: break
                    if server.poll() is not None: raise RuntimeError('nested display startup failed')
                    time.sleep(0.1)
                else: raise RuntimeError('nested display timeout')
                print('Isolated native display ready; only synthetic clipboard contents are used.', flush=True)
                return subprocess.call(['dbus-run-session', '--', '/usr/bin/python3', __file__, '--inside',*([mode] if mode else [])], env=env)
            finally: stop(server)
if __name__ == '__main__': sys.exit(main())
