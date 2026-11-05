$code = @"
using System;
using System.IO.MemoryMappedFiles;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public class DbgListener : IDisposable
{
    private EventWaitHandle bufferReady;
    private EventWaitHandle dataReady;
    private MemoryMappedFile mmf;
    private MemoryMappedViewAccessor accessor;
    private bool disposed = false;

    public DbgListener()
    {
        bufferReady = new EventWaitHandle(false, EventResetMode.AutoReset, "DBWIN_BUFFER_READY");
        dataReady = new EventWaitHandle(false, EventResetMode.AutoReset, "DBWIN_DATA_READY");
        mmf = MemoryMappedFile.CreateNew("DBWIN_BUFFER", 4096);
        accessor = mmf.CreateViewAccessor();
        bufferReady.Set();
    }

    public string Read(int timeoutMs)
    {
        if (!dataReady.WaitOne(timeoutMs))
        {
            return null;
        }

        int pid = accessor.ReadInt32(0);
        byte[] bytes = new byte[4092];
        accessor.ReadArray(4, bytes, 0, 4092);

        int len = 0;
        while (len < bytes.Length && bytes[len] != 0) len++;
        string msg = Encoding.Default.GetString(bytes, 0, len);

        bufferReady.Set();
        return string.Format("[PID:{0}] {1}", pid, msg);
    }

    public void Dispose()
    {
        if (!disposed)
        {
            disposed = true;
            if (accessor != null) accessor.Dispose();
            if (mmf != null) mmf.Dispose();
            if (bufferReady != null) bufferReady.Dispose();
            if (dataReady != null) dataReady.Dispose();
        }
    }
}
"@

Add-Type -TypeDefinition $code

Write-Host "========================================================" -ForegroundColor Cyan
Write-Host " Listening for Sumibi-TIP logs (Press Ctrl+C to stop)..." -ForegroundColor Cyan
Write-Host " Open Notepad, switch to Sumibi, and type / press Ctrl+J." -ForegroundColor Yellow
Write-Host "========================================================" -ForegroundColor Cyan

$listener = $null
try {
    $listener = New-Object DbgListener
    while ($true) {
        $msg = $listener.Read(500)
        if ($msg -ne $null -and $msg -match "Sumibi") {
            Write-Host $msg -ForegroundColor Green
        }
    }
} catch {
    Write-Error $_
} finally {
    if ($listener -ne $null) {
        $listener.Dispose()
    }
}
