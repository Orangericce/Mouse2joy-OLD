#!/usr/bin/env python3
"""
mouse2joy.py — Turn a mouse into a virtual joystick axis pair on Linux.

Reads relative mouse movement from a given /dev/input/eventX device and
emits it as a self-centering (spring-back) joystick via uinput. Designed
for flight-sim style camera/aim control (e.g. DCS World) where you want
the stick to snap back to center when you stop moving the mouse, rather
than drifting forever like raw relative mouse input would.

Usage:
    sudo python3 mouse2joy.py

  (needs to read /dev/input/eventX directly — see permissions note below)

Controls (once running):
    Press mouse button 5 (side, lower) -> toggles "grabbed" mode on: mouse
                                 deltas drive the virtual stick axes.
    Press it again               -> toggles grabbed mode off: axes spring
                                 back to center, mouse behaves normally
                                 again (you're not fighting your desktop
                                 cursor).
    Ctrl+C                       -> quit cleanly.

Tuning knobs are the CONSTANTS block below.
"""

import time
import threading

import uinput
from evdev import InputDevice, ecodes, list_devices

# ---------------------------------------------------------------------------
# CONFIG — tune these to taste
# ---------------------------------------------------------------------------

# Your mouse's event device (Compx VXE NordicMouse 1K Dongle), referenced
# via its stable /dev/input/by-id/ symlink rather than a raw eventN path
# — this stays correct across reboots and replugging, unlike a bare
# /dev/input/eventN which can silently point at a different device.
MOUSE_DEVICE_PATH = (
    "/dev/input/by-id/usb-Compx_VXE_NordicMouse_1K_Dongle-if02-event-mouse"
)

# Axis range emitted to the virtual joystick. DCS/most games expect a
# signed range; -32767..32767 is the common joystick convention.
AXIS_MIN = -32767
AXIS_MAX = 32767

# How many "joystick units" one unit of raw mouse movement is worth.
# Raise this if the stick feels sluggish, lower it if tiny mouse moves
# slam the axis to full deflection.
SENSITIVITY = 1000.0

# Spring-back strength: fraction of the remaining distance-to-center that
# gets pulled back per centering tick (0.0 = no centering, 1.0 = snaps
# instantly). X and Y are separate so pitch (Y) can linger longer than
# roll (X) — useful for flight sims where pitch authority feels different
# from roll. Runs at CENTER_TICK_HZ.
CENTER_STRENGTH_X = 0.04   # roll — snaps back relatively quickly
CENTER_STRENGTH_Y = 0.02   # pitch — lingers longer before returning

# How often the centering pull is applied, in Hz.
CENTER_TICK_HZ = 60

# Which mouse button acts as the grab toggle. Confirmed via evtest:
# the lower side button reports as BTN_SIDE on this mouse.
GRAB_BUTTON = ecodes.BTN_SIDE

# True  = spring-centering runs the whole time you're grabbed, so holding
#         the mouse still at a deflected position still decays toward 0.
#         Feels like a self-centering flight stick that returns whenever
#         you're not actively pushing it — good for pitch/roll/rudder axes.
# False = position holds steady while grabbed; centering only kicks in
#         after you release the grab button. Feels like a "look" axis
#         (e.g. TrackIR-style view control) that stays where you left it.
CENTER_WHILE_GRABBED = True

# ---------------------------------------------------------------------------


def clamp(value, lo, hi):
    return max(lo, min(hi, value))


def find_device(path):
    try:
        return InputDevice(path)
    except (FileNotFoundError, PermissionError) as exc:
        print(f"Could not open {path}: {exc}")
        print("\nAvailable input devices:")
        for p in list_devices():
            try:
                d = InputDevice(p)
                print(f"  {p}\t{d.name}")
            except (FileNotFoundError, PermissionError) as inner_exc:
                print(f"  {p}\t(could not read: {inner_exc})")
        if isinstance(exc, PermissionError):
            print(
                "\nLooks like a permissions issue — see the 'give yourself "
                "read access' step (add yourself to the 'input' group)."
            )
        raise SystemExit(1)


def main():
    mouse = find_device(MOUSE_DEVICE_PATH)
    print(f"Reading from: {mouse.path} ({mouse.name})")

    joystick = uinput.Device(
        [
            uinput.ABS_X + (AXIS_MIN, AXIS_MAX, 0, 0),
            uinput.ABS_Y + (AXIS_MIN, AXIS_MAX, 0, 0),
            uinput.BTN_TRIGGER,  # required: SDL2 won't classify a uinput
            uinput.BTN_THUMB,    # device as a joystick without at least
            uinput.BTN_THUMB2,   # one button capability present
        ],
        name="mouse2joy Virtual Joystick",
    )
    print("Virtual joystick created: 'mouse2joy Virtual Joystick'")
    print(f"Grab button: {ecodes.BTN[GRAB_BUTTON]}  (press to toggle)")
    print("Ctrl+C to quit.\n")

    state = {
        "x": 0.0,
        "y": 0.0,
        "grabbed": False,
        "running": True,
    }
    lock = threading.Lock()

    def centering_loop():
        interval = 1.0 / CENTER_TICK_HZ
        while state["running"]:
            time.sleep(interval)
            with lock:
                should_center = CENTER_WHILE_GRABBED or not state["grabbed"]
                if should_center:
                    state["x"] -= state["x"] * CENTER_STRENGTH_X
                    state["y"] -= state["y"] * CENTER_STRENGTH_Y
                x = int(clamp(state["x"], AXIS_MIN, AXIS_MAX))
                y = int(clamp(state["y"], AXIS_MIN, AXIS_MAX))
            joystick.emit(uinput.ABS_X, x, syn=False)
            joystick.emit(uinput.ABS_Y, y)

    threading.Thread(target=centering_loop, daemon=True).start()

    try:
        for event in mouse.read_loop():
            if event.type == ecodes.EV_KEY and event.code == GRAB_BUTTON:
                if event.value == 1:  # button press only — ignore release
                    with lock:
                        state["grabbed"] = not state["grabbed"]
                        grabbed_now = state["grabbed"]
                    if grabbed_now:
                        mouse.grab()   # exclusively capture mouse — raw
                        print("Grabbed (mouse hidden from other apps)")
                    else:
                        mouse.ungrab() # release back to OS
                        print("Released")
                continue

            if not state["grabbed"]:
                continue

            if event.type == ecodes.EV_REL:
                with lock:
                    if event.code == ecodes.REL_X:
                        state["x"] = clamp(
                            state["x"] + event.value * SENSITIVITY / 100.0,
                            AXIS_MIN,
                            AXIS_MAX,
                        )
                    elif event.code == ecodes.REL_Y:
                        state["y"] = clamp(
                            state["y"] + event.value * SENSITIVITY / 100.0,
                            AXIS_MIN,
                            AXIS_MAX,
                        )
    except KeyboardInterrupt:
        pass
    finally:
        state["running"] = False
        try:
            mouse.ungrab()
        except Exception:
            pass
        print("\nShutting down, centering axes...")
        joystick.emit(uinput.ABS_X, 0, syn=False)
        joystick.emit(uinput.ABS_Y, 0)
        time.sleep(0.1)


if __name__ == "__main__":
    main()
