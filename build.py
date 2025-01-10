#!/usr/bin/env python3

import os, argparse, subprocess, sys

if sys.platform == 'linux':
    LIB = 'libnvim_repolink.so'
    OUT = 'nvim_repolink.so'
elif sys.platform == 'win32':
    LIB = 'nvim_repolink.dll'
    OUT = LIB
elif sys.platform == 'darwin':
    LIB = 'libnvim_repolink.dylib'
    OUT = 'nvim_repolink.so'

def cmd(x):
    try:
        p = subprocess.Popen(x.split(), stdin=sys.stdin, stdout=sys.stdout, stderr=sys.stderr, text=True)
        p.wait()
    except Exception as e:
        print(f'{e}')

def build(args):
    flags = '' if args.debug_build else ' --release'
    src = os.path.join('target', 'debug' if args.debug_build else 'release', LIB)
    cmd('cargo build' + flags)
    try:
        os.rename(src, OUT)
    except OSError as e:
        print(f'{e}')

def clean(args):
    cmd('cargo clean')
    try:
        os.remove(OUT)
    except FileNotFoundError as e:
        print(f'{e}')

def main():
    p = argparse.ArgumentParser()
    p.add_argument('-d', '--debug-build', default=False, action='store_true')
    p.add_argument('action')
    args = p.parse_args()
    actions = {'build': build, 'clean': clean}
    def usage():
        print(f'{sys.argv[0]} {"|".join(actions.keys())}', file=sys.stderr)

    try:
        actions[args.action](args)
    except KeyError:
        usage()

if __name__ == '__main__':
    main()
