$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
try {
    $request = [Console]::In.ReadToEnd() | ConvertFrom-Json
    if ($request.operation -eq 'network') {
        $gateway = [Net.IPAddress]::Parse([string]$request.gateway)
        $address = [Net.IPAddress]::Parse([string]$request.interface_ip)
        if ($gateway.AddressFamily -ne [Net.Sockets.AddressFamily]::InterNetwork -or $address.AddressFamily -ne $gateway.AddressFamily) { throw 'IPv4 required' }
        $interfaces = @(Get-NetIPAddress -AddressFamily IPv4 -IPAddress $address.ToString())
        if ($interfaces.Count -ne 1) { throw 'Interface identity is ambiguous' }
        $index = $interfaces[0].InterfaceIndex
        if ($request.action -eq 'prepare') {
            $existing = @(Get-NetRoute -AddressFamily IPv4 -DestinationPrefix '0.0.0.0/0' -ErrorAction SilentlyContinue)
            $created = $existing.Count -eq 0
            if ($created) {
                New-NetRoute -DestinationPrefix '0.0.0.0/0' -InterfaceIndex $index -NextHop $gateway.ToString() -RouteMetric 42317 -PolicyStore ActiveStore | Out-Null
            }
            @{ok=$true; created=$created; gateway=$gateway.ToString()} | ConvertTo-Json -Compress
        } elseif ($request.action -eq 'restore') {
            Get-NetRoute -DestinationPrefix '0.0.0.0/0' -InterfaceIndex $index -PolicyStore ActiveStore -ErrorAction SilentlyContinue |
                Where-Object { $_.NextHop -eq $gateway.ToString() -and $_.RouteMetric -eq 42317 } |
                Remove-NetRoute -Confirm:$false
            @{ok=$true} | ConvertTo-Json -Compress
        } else { throw 'Invalid network operation' }
        exit 0
    }
    if ($request.operation -in @('stage', 'install')) {
        $digest = [string]$request.sha256
        if ($digest -notmatch '^[0-9a-f]{64}$') { throw 'Invalid installer digest' }
        $stage = 'C:\ProgramData\P2PRemoteRegression\staging'
        [IO.Directory]::CreateDirectory($stage) | Out-Null
        $installer = Join-Path $stage ($digest + '.exe')
        if ($request.operation -eq 'stage') {
            @{ok=$true; path=$installer.Replace('\','/')} | ConvertTo-Json -Compress
            exit 0
        }
        if ((Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant() -ne $digest) {
            throw 'Installer digest mismatch'
        }
        $setup = Start-Process -FilePath $installer -ArgumentList '/S' -WindowStyle Hidden -PassThru
        if (-not $setup.WaitForExit(180000)) { $setup.Kill(); throw 'Installer timeout' }
        @{ok=($setup.ExitCode -eq 0); exit_code=$setup.ExitCode; installer_sha256=$digest} | ConvertTo-Json -Compress
        exit 0
    }
    $cli = [string]$request.cli_path
    if (-not [IO.Path]::IsPathRooted($cli) -or -not (Test-Path -LiteralPath $cli -PathType Leaf)) {
        throw 'CLI unavailable'
    }
    if ($request.operation -eq 'inventory') {
        @{ok=$true; cli_sha256=(Get-FileHash -LiteralPath $cli -Algorithm SHA256).Hash.ToLowerInvariant()} | ConvertTo-Json -Compress
        exit 0
    }
    $processInfo = New-Object Diagnostics.ProcessStartInfo
    $processInfo.FileName = $cli
    $processInfo.UseShellExecute = $false
    $processInfo.CreateNoWindow = $true
    $processInfo.RedirectStandardInput = $true
    $processInfo.RedirectStandardOutput = $true
    $processInfo.RedirectStandardError = $true
    $processInfo.StandardOutputEncoding = New-Object System.Text.UTF8Encoding($false)
    $processInfo.StandardErrorEncoding = New-Object System.Text.UTF8Encoding($false)
    if ($request.operation -eq 'service') {
        if ($request.action -notin @('start','stop','restart')) { throw 'Invalid service operation' }
        $processInfo.Arguments = 'service ' + $request.action
        $seconds = 90
    } elseif ($request.operation -eq 'request') {
        $seconds = [int]$request.timeout_secs
        if ($seconds -lt 1 -or $seconds -gt 300) { throw 'Invalid timeout' }
        $processInfo.Arguments = 'automation --timeout-secs ' + $seconds
    } else {
        throw 'Unknown operation'
    }
    $process = [Diagnostics.Process]::Start($processInfo)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    if ($request.operation -eq 'request') {
        # Windows PowerShell 5.1 lacks ProcessStartInfo.StandardInputEncoding.
        # Write UTF-8 directly so account names/passwords do not depend on the VM code page.
        $payload = [Text.Encoding]::UTF8.GetBytes(($request.command | ConvertTo-Json -Depth 30 -Compress))
        $process.StandardInput.BaseStream.Write($payload, 0, $payload.Length)
        $process.StandardInput.BaseStream.Flush()
    }
    $process.StandardInput.Close()
    if (-not $process.WaitForExit(($seconds + 10) * 1000)) { $process.Kill(); throw 'CLI timeout' }
    if ($request.operation -eq 'service') {
        @{ok=($process.ExitCode -eq 0); exit_code=$process.ExitCode} | ConvertTo-Json -Compress
    } else {
        $response = $stdout.Result | ConvertFrom-Json
        if ($null -eq $response.ok) { throw 'Invalid CLI response' }
        if ($process.ExitCode -ne 0 -and $response.ok -eq $true) { throw 'CLI exit code contradicts response' }
        $response | ConvertTo-Json -Depth 40 -Compress
    }
} catch {
    # Exception text and child stderr may contain credential-bearing input.
    @{ok=$false; error_code='regression.agent.failed'} | ConvertTo-Json -Compress
    exit 1
}
