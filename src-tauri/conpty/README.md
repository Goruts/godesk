# ConPTY for Windows

`conpty.dll` and `OpenConsole.exe` (x64) from the NuGet package
[Microsoft.Windows.Console.ConPTY](https://www.nuget.org/packages/Microsoft.Windows.Console.ConPTY)
1.25.260930003, the console host of [Windows Terminal](https://github.com/microsoft/terminal).
Both are signed by Microsoft. MIT License, Copyright (c) Microsoft Corporation.

`portable-pty` loads a `conpty.dll` found beside the executable before the
one built into Windows (`src/win/psuedocon.rs`), and that `conpty.dll` starts
the `OpenConsole.exe` beside it. Windows 10's own ConPTY drops lines when
output scrolls faster than it renders: an agent's earlier conversation never
reached the scrollback. `tauri.windows.conf.json` installs both next to
`godesk.exe`. `pty::windows_tests::fast_output_reaches_the_terminal_line_by_line`
(ignored) measures it: copy both files beside the test binary and run it with
`--ignored`.

| File | SHA-256 |
|---|---|
| `conpty.dll` | `feeef341d891643c62d30b6b07800bc70f0bc148f44cb8c3bee84aa557ae805a` |
| `OpenConsole.exe` | `3d66b23d0a71bb8eed2b77edc8b9df9bf54ce6c8fb74c863a30e760997f80586` |
