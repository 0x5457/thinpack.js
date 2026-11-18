# Records docs/demo.gif for the README: run `vhs docs/demo.tape` from the repository root
# with an installed `thinpack` (vhs lives at github.com/charmbracelet/vhs).
Output docs/demo.gif

Set Shell "bash"
Set FontSize 16
Set Width 1100
Set Height 520
Set Padding 24
Set LineHeight 1.3
Set TypingSpeed 55ms
Set Theme "Catppuccin Mocha"

Type "thinpack pack examples/hello-http.js -o hello-http"
Enter
Sleep 3s

Type "./hello-http"
Enter
Sleep 2s

Type "ls -lh hello-http"
Enter
Sleep 2s
