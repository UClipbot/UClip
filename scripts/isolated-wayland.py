#!/usr/bin/python3
"""Verify explicit restrictions in a disposable native Wayland compositor, with no XWayland."""
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time


def main():
    with tempfile.TemporaryDirectory(prefix='uclip-wayland-') as directory:
        root=Path(directory)
        runtime=root/'runtime'
        runtime.mkdir(mode=0o700)
        env=os.environ.copy()
        env.update(XDG_RUNTIME_DIR=str(runtime),WAYLAND_DISPLAY='uclip-test',XDG_SESSION_TYPE='wayland',GDK_BACKEND='wayland',NO_AT_BRIDGE='1')
        for key in ['DISPLAY','XAUTHORITY','DBUS_SESSION_BUS_ADDRESS','GDK_SCALE','GDK_DPI_SCALE']:
            env.pop(key,None)
        for key,name in [('XDG_DATA_HOME','data'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache')]:
            env[key]=str(root/name)
        with (root/'weston.log').open('w+') as log:
            compositor=subprocess.Popen(['weston','--backend=headless','--renderer=pixman','--socket=uclip-test','--idle-time=0','--no-config'],env=env,stdout=log,stderr=log,start_new_session=True)
            try:
                for _ in range(100):
                    if (runtime/'uclip-test').exists():break
                    if compositor.poll() is not None:raise RuntimeError('Weston failed')
                    time.sleep(0.05)
                else:raise RuntimeError('Wayland socket timeout')
                return subprocess.call(['/usr/bin/python3','scripts/native-smoke.py','--wayland-restrictions'],env=env)
            except Exception:
                log.seek(0)
                print(log.read())
                raise
            finally:
                os.killpg(compositor.pid,signal.SIGTERM)
                try:compositor.wait(timeout=5)
                except subprocess.TimeoutExpired:os.killpg(compositor.pid,signal.SIGKILL);compositor.wait()


if __name__=='__main__':raise SystemExit(main())
