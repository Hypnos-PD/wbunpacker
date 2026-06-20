#!/usr/bin/env python3
"""Search game process memory for sqlite3mc key bytes.

Usage: Run Shadowverse WB to title screen first, then:
  python tools/hook_key.py

It scans the game process memory for a known final_key fragment
(derived from encrypted meta.db XOR "SQLite format 3\0") and
prints surrounding bytes.
"""

import ctypes
from ctypes import wintypes

FINAL_KEY_FRAG = bytes.fromhex("66c7ce719582ebea7595d66a39ff65ae")
PROCESS_VM_READ = 0x0010
PROCESS_QUERY_INFORMATION = 0x0400
TH32CS_SNAPPROCESS = 0x00000002

k32 = ctypes.WinDLL("kernel32", use_last_error=True)

def find_game_pid():
    snap = k32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    entry = wintypes.PROCESSENTRY32W()
    entry.dwSize = ctypes.sizeof(entry)
    if k32.Process32FirstW(snap, ctypes.byref(entry)):
        while True:
            if entry.szExeFile == "ShadowverseWB.exe":
                k32.CloseHandle(snap)
                return entry.th32ProcessID
            if not k32.Process32NextW(snap, ctypes.byref(entry)):
                break
    k32.CloseHandle(snap)
    return None

def scan(pid, needle):
    h = k32.OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, False, pid)
    if not h:
        print("Cannot open process (admin may be needed)")
        return
    si = wintypes.SYSTEM_INFO()
    k32.GetSystemInfo(ctypes.byref(si))
    lo, hi = si.lpMinimumApplicationAddress, si.lpMaximumApplicationAddress
    mbi = wintypes.MEMORY_BASIC_INFORMATION()
    buf = (ctypes.c_char * 65536)()
    addr = lo
    found = 0
    while addr < hi:
        if not k32.VirtualQueryEx(h, addr, ctypes.byref(mbi), ctypes.sizeof(mbi)):
            addr += 65536; continue
        if mbi.State != 0x1000 or mbi.Protect & 0x100:
            addr = mbi.BaseAddress + mbi.RegionSize; continue
        ra, re = mbi.BaseAddress, mbi.BaseAddress + mbi.RegionSize
        while ra < re:
            sz = min(65536, re - ra)
            nr = wintypes.SIZE_T()
            if k32.ReadProcessMemory(h, ra, buf, sz, ctypes.byref(nr)):
                data = bytes(buf[:nr.value])
                idx = 0
                while True:
                    idx = data.find(needle, idx)
                    if idx == -1: break
                    ma = ra + idx
                    cb = (ctypes.c_char * 256)()
                    cn = wintypes.SIZE_T()
                    if k32.ReadProcessMemory(h, max(ma-32, 0), cb, 256, ctypes.byref(cn)):
                        ctx = bytes(cb[:cn.value])
                        offset = min(idx, 32)
                        key_data = ctx[offset:offset+len(needle)+32]
                        print(f"Match at 0x{ma:X}: {key_data.hex()}")
                        # Try to show as base64 if printable
                        try:
                            import base64
                            b64 = base64.b64encode(key_data).decode()
                            print(f"  b64: {b64}")
                        except: pass
                        found += 1
                        if found >= 5:
                            print("(stopping after 5 matches)")
                            k32.CloseHandle(h); return
                    idx += 1
            ra += sz
        addr = mbi.BaseAddress + mbi.RegionSize
    k32.CloseHandle(h)
    if found == 0:
        print("No matches. Key may not be in committed memory yet.")

if __name__ == "__main__":
    pid = find_game_pid()
    if not pid:
        print("ShadowverseWB.exe not running. Start the game first.")
    else:
        print(f"PID={pid}, searching for {FINAL_KEY_FRAG.hex()}")
        scan(pid, FINAL_KEY_FRAG)
