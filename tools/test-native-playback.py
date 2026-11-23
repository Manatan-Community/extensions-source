"""Decode a production stream DTO using Manatan's macOS media_kit libmpv.

Set MANATAN_MPV_LIBRARY and DYLD_FRAMEWORK_PATH to local copies of the
frameworks supplied by the app's media_kit_libs_macos_video dependency. Input
is JSON on stdin: {url, headers}. No URL, cookie, or header is logged.
"""
import ctypes as c
import json
import os
import sys
import time
import urllib.request
import urllib.parse
import urllib.error


class MpvEvent(c.Structure):
    _fields_ = [("event_id", c.c_int), ("error", c.c_int), ("reply_userdata", c.c_uint64), ("data", c.c_void_p)]


class MpvEndFile(c.Structure):
    _fields_ = [("reason", c.c_int), ("error", c.c_int)]


def main():
    request = json.load(sys.stdin)
    library = c.CDLL(os.environ["MANATAN_MPV_LIBRARY"])
    library.mpv_create.restype = c.c_void_p
    library.mpv_set_option_string.argtypes = [c.c_void_p, c.c_char_p, c.c_char_p]
    library.mpv_initialize.argtypes = [c.c_void_p]
    library.mpv_command.argtypes = [c.c_void_p, c.POINTER(c.c_char_p)]
    library.mpv_get_property.argtypes = [c.c_void_p, c.c_char_p, c.c_int, c.c_void_p]
    library.mpv_terminate_destroy.argtypes = [c.c_void_p]
    library.mpv_wait_event.argtypes = [c.c_void_p, c.c_double]
    library.mpv_wait_event.restype = c.POINTER(MpvEvent)
    library.mpv_error_string.argtypes = [c.c_int]
    library.mpv_error_string.restype = c.c_char_p
    player = library.mpv_create()
    if not player:
        raise RuntimeError("libmpv initialization failed")
    try:
        for name, value in [("vo", "null"), ("ao", "null"), ("hwdec", "no"),
                            ("terminal", "no"), ("idle", "yes"),
                            ("keep-open", "yes"), ("msg-level", "all=no")]:
            if library.mpv_set_option_string(player, name.encode(), value.encode()) < 0:
                raise RuntimeError("Unsupported native player option: " + name)
        headers = request.get("headers", [])
        if isinstance(headers, dict):
            headers = [{"name": k, "value": v} for k, v in headers.items()]
        # libmpv's option list uses commas; quote commas/backslashes in values.
        header_value = ",".join((h["name"] + ": " + h["value"]).replace("\\", "\\\\").replace(",", "\\,") for h in headers)
        if header_value:
            library.mpv_set_option_string(player, b"http-header-fields", header_value.encode())
        if library.mpv_initialize(player) < 0:
            raise RuntimeError("libmpv initialization failed")
        command = (c.c_char_p * 3)(b"loadfile", request["url"].encode(), None)
        if library.mpv_command(player, command) < 0:
            raise RuntimeError("Native player rejected loadfile")
        deadline = time.monotonic() + 75
        while time.monotonic() < deadline:
            position, frames, width = c.c_double(), c.c_int64(), c.c_int64()
            library.mpv_get_property(player, b"time-pos", 5, c.byref(position))
            library.mpv_get_property(player, b"estimated-frame-number", 4, c.byref(frames))
            library.mpv_get_property(player, b"video-params/w", 4, c.byref(width))
            if position.value >= 3 and frames.value >= 30 and width.value > 0:
                print(json.dumps({"ok": True, "seconds": round(position.value, 2), "frames": frames.value, "width": width.value}))
                return
            while True:
                event = library.mpv_wait_event(player, 0).contents
                if event.event_id == 0:
                    break
                if event.event_id == 7 and event.data:
                    end = c.cast(event.data, c.POINTER(MpvEndFile)).contents
                    detail = library.mpv_error_string(end.error).decode() if end.error else "end of file"
                    # Inspect only the local production proxy, never replay a
                    # signed external URL through a different network client.
                    if urllib.parse.urlsplit(request["url"]).hostname == "127.0.0.1":
                        try:
                            probe = urllib.request.Request(request["url"], headers={**{h["name"]: h["value"] for h in headers}, "Range": "bytes=0-1023"})
                            with urllib.request.urlopen(probe, timeout=10) as response:
                                detail += "; proxy HTTP " + str(response.status)
                        except urllib.error.HTTPError as error:
                            body = error.read(4000).decode(errors="replace")
                            try:
                                message = json.loads(body)["error"]["message"]
                            except (ValueError, KeyError, TypeError):
                                message = "request rejected"
                            detail += f"; proxy HTTP {error.code}: {message}"
                        except Exception:
                            detail += "; proxy diagnostic request did not complete"
                    raise RuntimeError("Native player ended before decoding: " + detail)
            time.sleep(0.1)
        raise RuntimeError("Native player did not decode 3 seconds / 30 frames within 75 seconds")
    finally:
        library.mpv_terminate_destroy(player)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(json.dumps({"ok": False, "error": str(error)}))
        sys.exit(1)
