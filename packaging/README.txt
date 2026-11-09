RizRef — a reference-image board for artists
=============================================

Portable use
------------
rizref.exe is the whole application. It needs no installer, no runtime and
no DLLs beyond Windows itself (Windows 10 or later, with a Vulkan or
DirectX 12 capable GPU driver). Copy it anywhere and run it.

Optional: Windows integration
-----------------------------
To open .rizref boards with a double-click, give them the RizRef icon and
show their thumbnails in Explorer, run once from a terminal in this folder:

    rizref.exe register

This writes per-user registry keys only (no administrator rights). Keep
rizref_thumb.dll next to rizref.exe: it is the Explorer thumbnail provider,
registered by that command when it is present. If you move the folder, run
"register" again. To undo it:

    rizref.exe unregister

Where RizRef keeps things
-------------------------
Settings and recent boards: %APPDATA%\RizRef
Logs, crash dumps, autosave copies: %LOCALAPPDATA%\RizRef
(Help -> Open logs folder opens it.)

If RizRef ever closes unexpectedly, the next start offers to restore the
work autosaved in the last minute.

Command line
------------
rizref.exe --help        all options
rizref.exe inspect B.rizref --verify
                         check a board file's integrity
