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

Installer
---------
RizRef-<version>-x64-setup.exe installs for the current user by default,
into %LOCALAPPDATA%\Programs\RizRef, without administrator rights. Its first
dialog also offers "Install for all users", which needs administrator
rights and defaults to Program Files.

Setup always asks where to install, including when upgrading (it starts
from the folder the previous version used). A per-user install needs a
folder you can write to without administrator rights; Setup says so before
copying anything if you pick one it cannot write to, such as Program Files.
A network folder works, but Explorer usually will not load the thumbnail
provider from one, so Setup asks first. Picking a different folder when
upgrading leaves the old one behind: uninstall first to move RizRef.

Silent install into a chosen folder:

    RizRef-<version>-x64-setup.exe /VERYSILENT /DIR="D:\Apps\RizRef"

Add /ALLUSERS for an all-users install (run elevated), or /CURRENTUSER to
force a per-user one. With the "associate" task (on by default) Setup runs
"rizref.exe register" from the chosen folder, so double-click, the file
icon and Explorer thumbnails point there; an all-users install registers
for every user ("register --machine"). Uninstalling (Settings -> Apps)
runs "rizref.exe unregister" and removes the folder.

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
