#!/usr/bin/env python3
"""ssh/scp stand-in when the local OpenSSH cannot run (no passwd entry).
usage: GPU_HOST=<ip> rsh.py run 'command'
       rsh.py get REMOTE_DIR LOCAL_DIR      (recursive)"""
import os
import stat
import sys
from pathlib import Path

import paramiko

HOST, USER = os.environ["GPU_HOST"], os.environ.get("GPU_USER", "ubuntu")
c = paramiko.SSHClient()
c.set_missing_host_key_policy(paramiko.AutoAddPolicy())
c.connect(HOST, username=USER, key_filename=os.path.expanduser("~/.ssh/id_ed25519"), timeout=20)

if sys.argv[1] == "run":
    _, out, err = c.exec_command(sys.argv[2])
    sys.stdout.write(out.read().decode())
    sys.stderr.write(err.read().decode())
    sys.exit(out.channel.recv_exit_status())
elif sys.argv[1] == "get":
    sftp = c.open_sftp()

    def get(r, l):
        Path(l).mkdir(parents=True, exist_ok=True)
        for e in sftp.listdir_attr(r):
            rp, lp = f"{r}/{e.filename}", os.path.join(l, e.filename)
            if stat.S_ISDIR(e.st_mode):
                get(rp, lp)
            else:
                sftp.get(rp, lp)

    get(sys.argv[2], sys.argv[3])
    print("got", sys.argv[2])
