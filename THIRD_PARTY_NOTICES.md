# Third-Party Notices

Bypath itself is dual-licensed under the MIT License or the Apache License, Version 2.0.

## WinDivert (Runtime Dynamic Dependency on Windows)

On Windows, Bypath dynamically loads `WinDivert.dll` at runtime (via `LoadLibrary` / `libloading`) and communicates with the signed `WinDivert64.sys` driver.

- Project: WinDivert (Windows Packet Divert)
- Copyright: Copyright (C) basil00 / reqrypt.org
- License: GNU Lesser General Public License v3.0 (LGPLv3) or GNU General Public License v2.0 (GPLv2)
- Official Source Code: https://github.com/basil00/WinDivert
- Official Releases: https://github.com/basil00/WinDivert/releases

Because `WinDivert.dll` is dynamically loaded rather than statically linked, users may freely replace `WinDivert.dll` and `WinDivert64.sys` in the runtime directory with any compatible or modified build in accordance with LGPLv3 Section 4d.

When redistributing binary releases that bundle `WinDivert.dll` and `WinDivert64.sys`, include the full WinDivert LGPLv3/GPLv3 license texts alongside this notice.
