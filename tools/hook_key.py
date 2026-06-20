#!/usr/bin/env python3
"""Hook sqlite3_key_v2 in a running ShadowverseWB process.

The game uses AES-256-OFB (not XOR), so we can't search for
keystream. Instead we search for the sqlite3_key_v2 function
entry bytes, then dump nearby memory for the key argument.

Usage: Run game to title screen, then:
  python tools/hook_key.py
"""

import ctypes
from ctypes import wintypes
import struct

PROCESS_VM_READ = 0x0010
PROCESS_VM_OPERATION = 0x0008
PROCESS_QUERY_INFORMATION = 0x0400
MEM_COMMIT = 0x1000

k32 = ctypes.WinDLL("kernel32", use_last_error=True)

def find_game_pid():
    snap = k32.CreateToolhelp32Snapshot(0x00000002, 0)
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

def get_module_base(pid, mod_name):
    """Get base address of a loaded module in the target process."""
    snap = k32.CreateToolhelp32Snapshot(0x00000008, pid)  # TH32CS_SNAPMODULE
    if snap == -1:
        return None
    entry = wintypes.MODULEENTRY32W()
    entry.dwSize = ctypes.sizeof(entry)
    if k32.Module32FirstW(snap, ctypes.byref(entry)):
        while True:
            if entry.szModule.lower() == mod_name.lower():
                k32.CloseHandle(snap)
                return entry.modBaseAddr
            if not k32.Module32NextW(snap, ctypes.byref(entry)):
                break
    k32.CloseHandle(snap)
    return None

def dump_module_memory(h, base, size):
    """Dump a module's memory and scan for a pattern."""
    buf = (ctypes.c_char * 65536)()
    addr = base
    remaining = size
    while remaining > 0:
        chunk = min(65536, remaining)
        nr = wintypes.SIZE_T()
        if k32.ReadProcessMemory(h, addr, buf, chunk, ctypes.byref(nr)):
            data = bytes(buf[:nr.value])
            yield addr, data
        addr += chunk
        remaining -= chunk

def find_sqlite_key_v2_function(dll_data, dll_base):
    """Find sqlite3_key_v2 in libnative.dll by scanning for its function prologue."""
    # The function prologue of sqlite3_key_v2 from the DLL:
    # We know the RVA from dumpbin: 0x10BD80
    # Let's use the known RVA since we have dumpbin output
    return dll_base + 0x10BD80

if __name__ == "__main__":
    pid = find_game_pid()
    if not pid:
        print("ShadowverseWB.exe not running. Start the game first.")
        exit(1)
    
    print(f"Found PID={pid}")
    
    libnative_base = get_module_base(pid, "libnative.dll")
    if not libnative_base:
        print("libnative.dll not loaded yet. Wait for the game to reach title screen.")
        exit(1)
    
    print(f"libnative.dll base: 0x{libnative_base:X}")
    
    h = k32.OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, False, pid)
    if not h:
        print("Cannot open process (run as admin)")
        exit(1)
    
    # Read the sqlite3_key_v2 function prologue to identify it
    key_v2_rva = 0x10BD80
    key_v2_addr = libnative_base + key_v2_rva
    fbuf = (ctypes.c_char * 64)()
    fnr = wintypes.SIZE_T()
    if k32.ReadProcessMemory(h, key_v2_addr, fbuf, 64, ctypes.byref(fnr)):
        prologue = bytes(fbuf[:fnr.value])
        print(f"sqlite3_key_v2 at 0x{key_v2_addr:X}: {prologue[:16].hex()}")
    
    # Scan the whole libnative.dll for references to sqlite3_key_v2
    # and dump strings nearby
    dll_size = 0x310000  # ~3MB from file size
    print(f"Scanning libnative.dll memory for keyword 'key'...")
    
    key_pattern = b'sqlite3_key'
    for addr, data in dump_module_memory(h, libnative_base, dll_size):
        idx = 0
        while True:
            idx = data.find(key_pattern, idx)
            if idx == -1: break
            match_addr = addr + idx
            ctx_buf = (ctypes.c_char * 256)()
            ctx_nr = wintypes.SIZE_T()
            if k32.ReadProcessMemory(h, match_addr - 32, ctx_buf, 256, ctypes.byref(ctx_nr)):
                ctx = bytes(ctx_buf[:ctx_nr.value])
                print(f"  'sqlite3_key' at 0x{match_addr:X}: {ctx[32:96].hex()}")
            idx += len(key_pattern)
    
    k32.CloseHandle(h)
    print("Done. Check above for key material near sqlite3_key references.")
