<#
.SYNOPSIS
  One read-only Codex review of an OxideLens change: builds the review package, writes the prompt, runs Codex.

.DESCRIPTION
  Call it with a literal path (`& 'C:\...\oxidelens\scripts\review\codex-review.ps1' ...`): a worktree-isolated agent
  session refuses `bash $VAR/script` and other computed command names, and accepts this form.

  Reads from -Workspace (the plan's working folder):  task-<N>-brief.md, task-<N>-report.md, constraints.md.
  Writes to -Workspace:  review-<base>..<head>.diff, <name>-prompt.md, <name>-result.md, <name>-stdout.txt,
  <name>-stderr.txt.  Prints the verdict (the result's first line) last.

  Kinds and their templates (next to this script):
    task      task.md      first review of a task's commits
    rereview  rereview.md  a fix round; -Findings names the earlier result file (in -Workspace)
    final     final.md     the whole branch; pass the plan, spec, ledger and triage list through -Extra

  Codex runs with `-s read-only`, `--ephemeral` and the working root at the repository. For the call it switches off
  the plugin-type features (plugins, apps, browser and computer use) and every MCP server defined in config.toml.

.EXAMPLE
  & $script -Kind task -Workspace $W -Task 3 -Base 29656b1 -Head 865cc36
.EXAMPLE
  & $script -Kind rereview -Workspace $W -Task 3 -Base 865cc36 -Head 86cff34 -Findings t3-result.md
.EXAMPLE
  & $script -Kind final -Workspace $W -Base 2f6aa9b -Head HEAD -Extra "- Plan: ``C:\...\plan.md``"
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)] [ValidateSet('task', 'rereview', 'final')] [string] $Kind,
  [Parameter(Mandatory)] [string] $Workspace,
  [Parameter(Mandatory)] [string] $Base,
  [Parameter(Mandatory)] [string] $Head,
  # The task number; names the brief, the report and (by default) the review.
  [int] $Task = 0,
  # The review's name (file prefix). Default: t<N> for a task, t<N>r<k> for a rereview, final / finalr<k> for final.
  [string] $Name,
  # rereview: the earlier result file, a name in -Workspace or a full path.
  [string] $Findings,
  # Markdown lines added under the template's requirements (background files, rulings, deferred findings to triage).
  [string] $Extra = '',
  [string] $Brief,
  [string] $Report,
  [string] $Constraints,
  [string] $Model = 'gpt-6-sol',
  # The repository to review; default the one holding the current directory (a worktree reviews its own branch).
  [string] $Repo,
  # Write the package and the prompt, then stop without calling Codex.
  [switch] $PromptOnly
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$utf8 = [Text.UTF8Encoding]::new($false)

function Fail([string] $Message) { Write-Error $Message -ErrorAction Continue; exit 2 }
function Invoke-Git { & git -C $Repo @args; if ($LASTEXITCODE -ne 0) { Fail "git $($args -join ' ') failed ($LASTEXITCODE)" } }
function InWorkspace([string] $Path) { if ([IO.Path]::IsPathRooted($Path)) { $Path } else { Join-Path $Workspace $Path } }

# --- Inputs ----------------------------------------------------------------------------------------------------------
if (-not $Repo) { $Repo = (& git rev-parse --show-toplevel); if ($LASTEXITCODE -ne 0) { Fail 'not inside a git repository; pass -Repo' } }
$Repo = [IO.Path]::GetFullPath($Repo)
$Workspace = [IO.Path]::GetFullPath($Workspace)
if (-not (Test-Path -LiteralPath $Workspace -PathType Container)) { Fail "no such workspace folder: $Workspace" }
if ($Kind -ne 'final' -and $Task -lt 1 -and -not ($Brief -and $Report -and $Name)) {
  Fail "-Kind $Kind needs -Task <N> (or -Name, -Brief and -Report, e.g. a re-review of the final fix wave)"
}
if ($Kind -eq 'rereview' -and -not $Findings) { Fail '-Kind rereview needs -Findings <earlier result file>' }

$standards = Join-Path $Repo 'oxidelens\CODING_STANDARDS.md'
$template = Join-Path $PSScriptRoot "$Kind.md"
if (-not $Brief) { $Brief = "task-$Task-brief.md" }
if (-not $Report) { $Report = "task-$Task-report.md" }
if (-not $Constraints) { $Constraints = 'constraints.md' }
$Brief = InWorkspace $Brief; $Report = InWorkspace $Report; $Constraints = InWorkspace $Constraints
$need = @($standards, $template, $Constraints)
if ($Kind -ne 'final') { $need += @($Brief, $Report) }
if ($Kind -eq 'rereview') { $Findings = InWorkspace $Findings; $need += $Findings }
foreach ($f in $need) { if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { Fail "missing input file: $f" } }

if (-not $Name) {
  $stem = if ($Kind -eq 'final') { 'final' } else { "t$Task" }
  if ($Kind -eq 'task' -or ($Kind -eq 'final' -and -not (Test-Path -LiteralPath (Join-Path $Workspace 'final-prompt.md')))) {
    $Name = $stem
  } else {
    $k = 1
    while (Test-Path -LiteralPath (Join-Path $Workspace "${stem}r$k-prompt.md")) { $k++ }
    $Name = "${stem}r$k"
  }
}

# --- Review package --------------------------------------------------------------------------------------------------
$b = (Invoke-Git rev-parse --verify --quiet --short "$Base^{commit}")
$h = (Invoke-Git rev-parse --verify --quiet --short "$Head^{commit}")
& git -C $Repo merge-base --is-ancestor $b $h
if ($LASTEXITCODE -ne 0) { Fail "HEAD is not a descendant of BASE: $b..$h" }
$commits = [int](Invoke-Git rev-list --count "$b..$h")
if ($commits -eq 0) { Fail "empty commit range: $b..$h" }

$package = Join-Path $Workspace "review-$b..$h.diff"
$diffPart = "$package.part"
Invoke-Git diff -U10 "--output=$diffPart" "$b..$h"
$preamble = @(
  "# Review package: $b..$h", '', '## Commits', (Invoke-Git log --oneline "$b..$h"), '',
  '## Files changed', (Invoke-Git diff --stat=160 "$b..$h"), '', '## Diff', ''
) | ForEach-Object { $_ }
[IO.File]::WriteAllText($package, ($preamble -join "`n") + "`n", $utf8)
$out = [IO.File]::Open($package, [IO.FileMode]::Append)
try { $in = [IO.File]::OpenRead($diffPart); try { $in.CopyTo($out) } finally { $in.Dispose() } } finally { $out.Dispose() }
Remove-Item -LiteralPath $diffPart

# --- Prompt ----------------------------------------------------------------------------------------------------------
$prompt = [IO.File]::ReadAllText($template)
$values = [ordered]@{
  BRIEF = $Brief; REPORT = $Report; CONSTRAINTS = $Constraints; STANDARDS = $standards; PACKAGE = $package
  FINDINGS = $(if ($Findings) { $Findings } else { '' }); BASE = $b; HEAD = $h; EXTRA = $Extra
}
foreach ($key in $values.Keys) { $prompt = $prompt.Replace("{{$key}}", [string] $values[$key]) }
if ($prompt -match '\{\{[A-Z]+\}\}') { Fail "unfilled placeholder in the prompt: $($Matches[0])" }
$promptFile = Join-Path $Workspace "$Name-prompt.md"
[IO.File]::WriteAllText($promptFile, $prompt, $utf8)

"package  $package  ($commits commit(s), $((Get-Item -LiteralPath $package).Length) bytes)"
"prompt   $promptFile"
if ($PromptOnly) { exit 0 }

# --- Codex -----------------------------------------------------------------------------------------------------------
$off = @()
$features = @(& codex features list 2>$null)
foreach ($feature in 'plugins', 'apps', 'browser_use', 'computer_use') {
  if ($features -match "^$feature\s") { $off += @('--disable', $feature) }
}
# Servers the features above bring are gone with them. Only a server defined in config.toml can be overridden: `-c` on
# any other makes a transport-less entry, and Codex refuses to start.
$config = Join-Path $(if ($env:CODEX_HOME) { $env:CODEX_HOME } else { Join-Path $HOME '.codex' }) 'config.toml'
if (Test-Path -LiteralPath $config) {
  $defined = [regex]::Matches([IO.File]::ReadAllText($config), '(?m)^\[mcp_servers\.([A-Za-z0-9_-]+)\]') |
    ForEach-Object { $_.Groups[1].Value }
  foreach ($server in @($defined | Select-Object -Unique)) { $off += @('-c', "mcp_servers.$server.enabled=false") }
}

$result = Join-Path $Workspace "$Name-result.md"
$stdout = Join-Path $Workspace "$Name-stdout.txt"
$stderr = Join-Path $Workspace "$Name-stderr.txt"
Remove-Item -LiteralPath $result -ErrorAction SilentlyContinue
$started = Get-Date
Get-Content -LiteralPath $promptFile -Raw |
  & codex exec -m $Model -s read-only -C $Repo --ephemeral @off -o $result - 1> $stdout 2> $stderr
$code = $LASTEXITCODE
$took = [int]((Get-Date) - $started).TotalSeconds

"codex    exit $code, ${took}s, model $Model"
"result   $result"
if (-not (Test-Path -LiteralPath $result) -or (Get-Item -LiteralPath $result).Length -eq 0) {
  "no result written; the end of $stderr`:"
  Get-Content -LiteralPath $stderr -Tail 15
  exit $(if ($code -ne 0) { $code } else { 1 })
}
"verdict  $((Get-Content -LiteralPath $result -TotalCount 1).Trim())"
exit $code
