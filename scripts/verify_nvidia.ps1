# Reads the stored NVIDIA credential (keyring = Windows Credential Manager) and
# verifies: 1) model listing serves, 2) a real chat completion succeeds.
# The key is NEVER printed — only HTTP status and short summaries.
$ErrorActionPreference = 'Stop'
Add-Type -Namespace Native -Name Cred -MemberDefinition @'
[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
public struct CREDENTIAL {
  public int Flags; public int Type; public string TargetName; public string Comment;
  public System.Runtime.InteropServices.ComTypes.FILETIME LastWritten;
  public uint CredentialBlobSize; public IntPtr CredentialBlob; public uint Persist;
  public uint AttributeCount; public IntPtr Attributes; public IntPtr TargetAlias; public IntPtr UserName;
}
[DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool CredReadW(string target, int type, int flags, out IntPtr credPtr);
[DllImport("advapi32.dll")] public static extern void CredFree(IntPtr cred);
'@
$ptr = [IntPtr]::Zero
if (-not [Native.Cred]::CredReadW('api_key.cameo_provider_nvidia', 1, 0, [ref]$ptr)) {
  Write-Output 'CREDREAD_FAILED'; exit 1
}
$credType = [Native.Cred].GetNestedType('CREDENTIAL')
# Read fixed offsets instead of marshaling the nested struct (x64 layout):
# 32 = CredentialBlobSize (uint32), 40 = CredentialBlob (pointer)
$blobSize = [System.Runtime.InteropServices.Marshal]::ReadInt32($ptr, 32)
$blobPtr  = [System.Runtime.InteropServices.Marshal]::ReadIntPtr($ptr, 40)
$bytes = New-Object byte[] $blobSize
[System.Runtime.InteropServices.Marshal]::Copy($blobPtr, $bytes, 0, $blobSize)
[Native.Cred]::CredFree($ptr)
$key = [System.Text.Encoding]::Unicode.GetString($bytes).Trim([char]0).Trim()
if (-not $key) { Write-Output 'EMPTY_KEY'; exit 1 }
Write-Output ("KEY_LOADED: yes (length {0}, {1}..." -f $key.Length, $key.Substring(0,3))

$h = @{ Authorization = "Bearer $key"; Accept = 'application/json' }

# 1) endpoint serving check
try {
  $m = Invoke-RestMethod -Uri 'https://integrate.api.nvidia.com/v1/models' -Headers $h -TimeoutSec 30
  Write-Output "MODELS_SERVED: yes (count $($m.data.Count))"
  $ids = $m.data | ForEach-Object { $_.id }
  Write-Output ('SERVED_MODELS: ' + (($ids -join ',')))
  $targets = @('openai/gpt-oss-20b','z-ai/glm-5.3-flash','z-ai/glm-5.3','deepseek-ai/deepseek-v4.1-flash','nvidia/nemotron-3-nano-omni-30b-a3b-reasoning','nvidia/nemotron-3.5-lightning-30b-a3b','moonshotai/kimi-k2.6','google/diffusiongemma-26b-a4b-it','mistralai/mistral-large-2-instruct','meta/llama-3.2-11b-vision-instruct','mistralai/mistral-7b-instruct-v0.3','nvidia/mistral-nemo-minitron-8b-8k-instruct','ibm/granite-3.0-8b-instruct','nvidia/nemotron-3-super-120b-a12b')
  $candidates = $targets | Where-Object { $ids -contains $_ }
  if (-not $candidates) { $candidates = $ids | Select-Object -First 3 }
  $pick = $candidates
} catch {
  Write-Output ("MODELS_FAILED: {0}" -f $_.Exception.Message); exit 1
}

# 2) live chat completion; stops at the first model the account can serve
$served_ok = $false
foreach ($model in $pick) {
  $body = @{
    model = $model
    messages = @(@{ role = 'user'; content = 'Reply with exactly: CAMEO_OK' })
    max_tokens = 16
    temperature = 0
  } | ConvertTo-Json -Depth 5
  try {
    $r = Invoke-RestMethod -Uri 'https://integrate.api.nvidia.com/v1/chat/completions' -Method Post -Headers $h -ContentType 'application/json' -Body $body -TimeoutSec 60
    $msg = $r.choices[0].message
    $txt = ($msg.content, $msg.reasoning_content, ($r.choices[0] | ConvertTo-Json -Depth 3)) | Where-Object { $_ } | Select-Object -First 1
    if ($txt -and $txt.ToString().Trim()) {
      Write-Output ("CHAT_OK model={0} reply={1}" -f $model, (($txt.ToString() -replace '\s+', ' ').Trim().Substring(0, [Math]::Min(60, $txt.ToString().Trim().Length))))
      $served_ok = $true
      break
    } else {
      Write-Output ("CHAT_EMPTY model={0} (HTTP {1}, model served but returned empty text)" -f $model, ($r.usage | ConvertTo-Json -Compress))
    }
  } catch {
    $detail = ''
    if ($_.ErrorDetails.Message) { $detail = ($_.ErrorDetails.Message | Select-Object -First 1) }
    Write-Output ("CHAT_FAIL model={0} err={1}" -f $model, ($detail -replace '\s+', ' ').Substring(0, [Math]::Min(160, $detail.Length)))
  }
}
