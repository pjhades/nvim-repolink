#!/usr/bin/env python3

import os, argparse, subprocess, sys, shutil

if sys.platform == 'linux':
    LIB = 'libnvim_repolink.so'
    OUT = 'nvim_repolink.so'
elif sys.platform == 'win32':
    LIB = 'nvim_repolink.dll'
    OUT = LIB
elif sys.platform == 'darwin':
    LIB = 'libnvim_repolink.dylib'
    OUT = 'nvim_repolink.so'
else:
    print('Unsupported platform')
    sys.exit(1)

LUA = 'lua'
OUT = os.path.join(LUA, OUT)

def cmd(x):
    try:
        p = subprocess.Popen(x.split(), stdin=sys.stdin, stdout=sys.stdout, stderr=sys.stderr, text=True)
        p.wait()
    except Exception as e:
        print(f'{e}')

def lib(args):
    return os.path.join('target', 'debug' if args.debug_build else 'release', LIB)

def build(args):
    flags = '' if args.debug_build else '--release'
    cmd(f'cargo build {flags}')
    try:
        os.mkdir(LUA)
    except FileExistsError:
        pass
    try:
        os.rename(lib(args), OUT)
    except OSError as e:
        print(f'{e}')

def clean(args):
    cmd('cargo clean')
    try:
        shutil.rmtree(LUA)
    except:
        pass

def main():
    p = argparse.ArgumentParser()
    p.add_argument('-d', '--debug-build', default=False, action='store_true')
    p.add_argument('action')
    args = p.parse_args()
    actions = {'build': build, 'clean': clean}
    def usage():
        print(f'{sys.argv[0]} {"|".join(actions.keys())}')
    try:
        actions[args.action](args)
    except KeyError:
        usage()

if __name__ == '__main__':
    main()
