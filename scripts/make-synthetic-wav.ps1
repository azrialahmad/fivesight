# Generates testdata/synthetic_en.wav: a two-voice (Zira/David) English exchange, 16 kHz mono.
# Only English SAPI voices ship with Windows, so this is a pipeline/perf fixture, not an accuracy test
# for Indonesian. Use real recordings for that.
$dir = Join-Path $PSScriptRoot '..\testdata'
New-Item -ItemType Directory -Force $dir | Out-Null
Add-Type -AssemblyName System.Speech
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
$fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
$lines = @(
  @('Microsoft Zira Desktop', 'Good morning everyone. Let us start with the status of ticket PAY-142, the checkout timeout issue.'),
  @('Microsoft David Desktop', 'Thanks. I reproduced it on staging yesterday. The retry logic in the payment service waits thirty seconds before failing.'),
  @('Microsoft Zira Desktop', 'Can you update the Confluence page with the root cause, and assign a fix to the backend team by Friday?'),
  @('Microsoft David Desktop', 'Sure. I will also add a regression test and let you know when the pull request is ready for review.')
)
$i = 0
foreach ($l in $lines) {
  $s.SelectVoice($l[0]); $s.SetOutputToWaveFile("$dir\part$i.wav", $fmt); $s.Speak($l[1]); $s.SetOutputToNull(); $i++
}
$s.Dispose()
$pcm = New-Object System.IO.MemoryStream
$sil = New-Object byte[] (16000 * 2 * 6 / 10)
for ($k = 0; $k -lt $i; $k++) {
  $b = [IO.File]::ReadAllBytes("$dir\part$k.wav")
  $pcm.Write($b, 44, $b.Length - 44); $pcm.Write($sil, 0, $sil.Length)
  Remove-Item "$dir\part$k.wav"
}
$data = $pcm.ToArray()
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter($out)
$w.Write([Text.Encoding]::ASCII.GetBytes('RIFF')); $w.Write([int](36 + $data.Length))
$w.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt ')); $w.Write([int]16); $w.Write([int16]1); $w.Write([int16]1)
$w.Write([int]16000); $w.Write([int]32000); $w.Write([int16]2); $w.Write([int16]16)
$w.Write([Text.Encoding]::ASCII.GetBytes('data')); $w.Write([int]$data.Length); $w.Write($data); $w.Flush()
[IO.File]::WriteAllBytes("$dir\synthetic_en.wav", $out.ToArray())
