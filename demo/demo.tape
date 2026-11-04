# The demo in the README, recorded with vhs (https://github.com/charmbracelet/vhs)
# against the made-up API from `cargo run --example mock-api`, so it never
# shows real data. To record it again after a change:
#
#   cargo run --example mock-api &
#   vhs demo/demo.tape

Output demo/demo.gif

Set Shell "bash"
Set FontSize 15
Set Width 1180
Set Height 660
Set Padding 18
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set CursorBlink false

Env INVOIST_BASE_URL "http://127.0.0.1:4010"
Env INVOIST_API_KEY "inv_sk_test_mock"

# A folder of made-up invoices to upload from.
Hide
Type "mkdir -p ~/invoices && cd ~/invoices && for f in northwind-2041.pdf harbor-print-0193.pdf; do printf '%PDF-1.4' > $f; done && clear" Enter
Show

Type "invoist documents list" Sleep 300ms Enter
Sleep 2.5s
Type "invoist documents get 86296ebf" Sleep 300ms Enter
Sleep 4s

Hide
Type "clear" Enter
Show
Type "invoist" Sleep 300ms Enter
Sleep 3s
Type "2" Sleep 2s
Down Sleep 1.5s
Down Sleep 1.5s
Type "3" Sleep 2.5s
Type ":" Sleep 600ms
Type "upl" Sleep 1.5s
Enter Sleep 2s
Down Sleep 600ms
Down Sleep 800ms
Enter Sleep 4s
Type "q"
Sleep 500ms
