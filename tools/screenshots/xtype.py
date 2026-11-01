"""Type keys and click into a window via XTest, only while that window has focus.

Usage: xtype.py <window-id> tokens...
  text       types the characters
  @Keysym    presses a keysym combo, e.g. @Control_L+Shift_L+s, @Return
  ~x,y       moves the pointer (window-relative physical pixels)
  !1 / ^1    presses / releases a mouse button;  #1 clicks it
  %0.3       sleeps
"""
import ctypes, re, subprocess, sys, time

x11 = ctypes.CDLL("libX11.so.6")
xtst = ctypes.CDLL("libXtst.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XStringToKeysym.restype = ctypes.c_ulong
x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x11.XFlush.argtypes = [ctypes.c_void_p]
x11.XGetInputFocus.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_int)]
xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]

d = x11.XOpenDisplay(None)
target = int(sys.argv[1], 16)
info = subprocess.run(["xwininfo", "-id", hex(target)], capture_output=True, text=True).stdout
ox = int(re.search(r"Absolute upper-left X:\s+(-?\d+)", info).group(1))
oy = int(re.search(r"Absolute upper-left Y:\s+(-?\d+)", info).group(1))


def guard():
    w = ctypes.c_ulong(); r = ctypes.c_int()
    x11.XGetInputFocus(d, ctypes.byref(w), ctypes.byref(r))
    if w.value != target:
        sys.exit(f"focus is {w.value:#x}, not {target:#x}; aborting")


def key(combo):
    guard()
    codes = [x11.XKeysymToKeycode(d, x11.XStringToKeysym(n.encode())) for n in combo.split("+")]
    if 0 in codes:
        sys.exit(f"unknown keysym in {combo!r}")
    for c in codes:
        xtst.XTestFakeKeyEvent(d, c, 1, 0)
    for c in reversed(codes):
        xtst.XTestFakeKeyEvent(d, c, 0, 0)
    x11.XFlush(d); time.sleep(0.03)


def button(b, down):
    guard()
    xtst.XTestFakeButtonEvent(d, b, 1 if down else 0, 0); x11.XFlush(d); time.sleep(0.05)


CHARS = {" ": "space", "-": "minus", ".": "period", "/": "slash", "|": "Shift_L+backslash", ";": "semicolon",
         "'": "apostrophe", "$": "Shift_L+4", "=": "equal", ",": "comma", "_": "Shift_L+minus", ":": "Shift_L+semicolon"}
for tok in sys.argv[2:]:
    if tok.startswith("~"):
        guard()
        x, y = map(int, tok[1:].split(","))
        xtst.XTestFakeMotionEvent(d, -1, ox + x, oy + y, 0); x11.XFlush(d); time.sleep(0.05)
    elif tok.startswith("!"):
        button(int(tok[1:]), True)
    elif tok.startswith("^"):
        button(int(tok[1:]), False)
    elif tok.startswith("#"):
        button(int(tok[1:]), True); button(int(tok[1:]), False)
    elif tok.startswith("%"):
        time.sleep(float(tok[1:]))
    elif tok.startswith("@"):
        key(tok[1:])
    else:
        for ch in tok:
            key(CHARS.get(ch, ("Shift_L+" + ch.lower()) if ch.isupper() else ch))
