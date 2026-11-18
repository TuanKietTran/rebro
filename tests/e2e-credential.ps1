# End-to-end: real git + real Windows Credential Manager + rebro as credential helper.
# Isolated: own REBRO_HOME, own credential-store service name, fake tokens, temp git config.
#   pwsh -File tests\e2e-credential.ps1 [-Exe path\to\rebro.exe]
param([string]$Exe = "$PSScriptRoot\..\target\debug\rebro.exe")
$ErrorActionPreference = 'Stop'
$Exe = (Resolve-Path $Exe).Path
$tmp = Join-Path $env:TEMP "rebro-e2e-$PID"
New-Item -ItemType Directory -Force $tmp | Out-Null
$env:REBRO_HOME = $tmp
$env:REBRO_KEYRING_SERVICE = 'rebro-e2e'
$env:GIT_CONFIG_GLOBAL = Join-Path $tmp 'gitconfig'      # never touch the real ~/.gitconfig
$env:GIT_CONFIG_NOSYSTEM = '1'                            # ignore Git Credential Manager from the system config
$env:REBRO_SKIP_TOOLS = '1'                            # don't hand fake tokens to the real gh/glab
$fail = 0

@'
[[account]]
forge = "github"
host = "github.com"
username = "me"
default = true

[[account]]
forge = "github"
host = "github.com"
username = "work-bot"
owners = ["acme"]

[[account]]
forge = "forgejo"
host = "codeberg.org"
username = "cb-me"
default = true
'@ | Set-Content (Join-Path $tmp 'accounts.toml')

# keyring's Windows backend stores generic credentials named "<user>.<service>"
$secrets = @{ 'github.com/me' = 'tok-me-1'; 'github.com/work-bot' = 'tok-work-2'; 'codeberg.org/cb-me' = 'tok-cb-3' }
foreach ($k in $secrets.Keys) { cmdkey /generic:"$k.rebro-e2e" /user:"$k" /pass:"$($secrets[$k])" | Out-Null }

try {
    & $Exe sync | Out-Host
    "--- git config written by sync:"; Get-Content $env:GIT_CONFIG_GLOBAL | Out-Host

    function Fill($hostName, $path) {
        $in = "protocol=https`nhost=$hostName`n" + ($(if ($path) { "path=$path`n" } else { '' })) + "`n"
        $out = $in | git credential fill 2>&1
        ($out | Where-Object { $_ -match '^(username|password)=' }) -join ' '
    }
    $cases = @(
        @('github.com', 'TuanKietTran/edge.git', 'username=me password=tok-me-1'),
        @('github.com', 'acme/app.git', 'username=work-bot password=tok-work-2'),
        @('github.com', 'me/x.git', 'username=me password=tok-me-1'),
        @('codeberg.org', 'cb-me/repo.git', 'username=cb-me password=tok-cb-3')
    )
    foreach ($c in $cases) {
        $got = Fill $c[0] $c[1]
        $ok = $got -eq $c[2]
        if (-not $ok) { $fail++ }
        "{0} {1}/{2} -> {3}" -f $(if ($ok) { 'ok  ' } else { 'FAIL' }), $c[0], $c[1], ($got -replace 'password=\S+', 'password=***')
    }
    # unknown host: rebro must stay silent (git would then prompt; GIT_TERMINAL_PROMPT=0 makes it fail fast)
    $env:GIT_TERMINAL_PROMPT = '0'
    $unk = "protocol=https`nhost=example.org`n`n" | & $Exe git-credential get
    if ($unk) { $fail++; "FAIL unknown host answered" } else { "ok   unknown host: no answer" }
    # store/erase must never touch managed tokens
    "protocol=https`nhost=github.com`nusername=me`npassword=WRONG`n`n" | & $Exe git-credential erase
    $after = Fill 'github.com' 'me/x.git'
    if ($after -eq 'username=me password=tok-me-1') { "ok   erase ignored, token intact" } else { $fail++; "FAIL erase changed the token" }
    & $Exe doctor | Out-Host
}
finally {
    foreach ($k in $secrets.Keys) { cmdkey /delete:"$k.rebro-e2e" | Out-Null }
    Remove-Item -Recurse -Force $tmp
}
if ($fail) { throw "$fail check(s) failed" }
"ALL PASSED"
