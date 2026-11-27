using System.Diagnostics;
using System.Reflection;
using System.ServiceProcess;
using System.Text.Json;
using System.Text.Json.Serialization;
using LibreHardwareMonitor.Hardware;

namespace Ctrlmate.LhmBridge;

// ---------------------------------------------------------------------------------------------
// Ctrlmate LibreHardwareMonitor bridge.
//
// Line protocol: one JSON object per line on stdout (daemon reads) and stdin (daemon writes).
// Every outgoing object has a "type":
//   hello      once at start        {version, lhm_version, pid, driver}
//   heartbeat  every second         {seq, ts_ms, poll_age_ms, polls}  (independent thread: the daemon
//                                    detects a dead bridge by silence and a hung poll by poll_age_ms)
//   snapshot   after each poll      {cpu_temp_c, cpu_tctl_c, cpu_package_c, cpu_ccd_temps_c[], ...}
//   ack        per command          {cmd, ok, detail?}
//   error      malformed input      {error}
//   anti_cheat anti-cheat detected  {processes[]}  (bridge then restores PWM and exits with code 4)
//   refused    refused to start     {reason}       (exit code 3)
//   stopped    last line            {pwm_reset, verified}
// Commands (stdin): set_all_pwm {percent}, set_pwm {id, percent}, reset_pwm, quit|stop.
//
// Exit codes: 0 clean, 3 refused (--no-driver / anti-cheat at start), 4 anti-cheat appeared, 5 init error.
// ---------------------------------------------------------------------------------------------

internal sealed class BridgeCommand
{
    public string? cmd { get; set; }
    public int? percent { get; set; }
    public string? id { get; set; }
}

internal sealed class CaseFanDto
{
    public string id { get; set; } = "";
    public string name { get; set; } = "";
    public double rpm { get; set; }
    public double? duty_pct { get; set; }
}

internal sealed class NamedValueDto
{
    public string name { get; set; } = "";
    public double value { get; set; }
}

internal sealed class ControlDto
{
    public string id { get; set; } = "";
    public string name { get; set; } = "";
    /// cpu_fan | pump | chassis | unknown. Only "chassis" is ever driven by set_all_pwm / set_pwm.
    public string kind { get; set; } = "unknown";
    public double? value_pct { get; set; }
    /// default (firmware) | software | undefined
    public string mode { get; set; } = "undefined";
}

internal sealed class SnapshotDto
{
    public string type { get; set; } = "snapshot";
    public double? cpu_temp_c { get; set; }
    public double? cpu_tctl_c { get; set; }
    public double? cpu_package_c { get; set; }
    public List<NamedValueDto> cpu_ccd_temps_c { get; set; } = new();
    public double? cpu_power_w { get; set; }
    public string? motherboard_name { get; set; }
    public double? motherboard_vrm_temp_c { get; set; }
    public double? motherboard_chipset_temp_c { get; set; }
    public double? ram_temp_c { get; set; }
    public List<CaseFanDto> case_fans { get; set; } = new();
    public List<ControlDto> controls { get; set; } = new();
}

internal static class Program
{
    internal const string Version = "0.7.0";

    /// Chassis fans are never commanded below this duty (documented floor).
    internal const int ChassisFanFloorPercent = 25;

    private static readonly object Sync = new();
    private static readonly object OutLock = new();
    private static readonly JsonSerializerOptions Json = new()
    {
        DefaultIgnoreCondition = JsonIgnoreCondition.Never,
        PropertyNamingPolicy = null,
    };

    private static volatile bool _stop;
    private static long _seq;
    private static long _polls;
    private static long _lastPollDoneTicks = Stopwatch.GetTimestamp();
    private static readonly HashSet<string> ExtraChassisIds = new(StringComparer.OrdinalIgnoreCase);

    public static int Main(string[] args)
    {
        bool noDriver = false;
        for (var i = 0; i < args.Length; i++)
        {
            if (args[i] == "--no-driver") noDriver = true;
            else if (args[i] == "--chassis-id" && i + 1 < args.Length) ExtraChassisIds.Add(args[++i]);
        }

        if (noDriver)
        {
            Emit(new { type = "refused", reason = "no_driver_flag" });
            return 3;
        }

        var present = AntiCheat.Detect();
        if (present.Count > 0)
        {
            Emit(new { type = "refused", reason = "anti_cheat_active", processes = present });
            return 3;
        }

        Computer computer;
        try
        {
            computer = new Computer
            {
                IsCpuEnabled = true,
                IsMotherboardEnabled = true,
                IsMemoryEnabled = true,
                IsGpuEnabled = false,
                IsStorageEnabled = false,
                IsNetworkEnabled = false,
                IsControllerEnabled = false,
            };
            computer.Open();
        }
        catch (Exception ex)
        {
            Emit(new { type = "error", error = "init_failed", detail = ex.Message });
            return 5;
        }

        if (LoadedWinRing0())
        {
            // LibreHardwareMonitorLib >= 0.9.5 uses PawnIO. A WinRing0 module means a wrong build: abort.
            Emit(new { type = "refused", reason = "winring0_loaded" });
            SafeShutdown(computer);
            return 5;
        }

        Emit(new
        {
            type = "hello",
            version = Version,
            lhm_version = typeof(Computer).Assembly.GetName().Version?.ToString(),
            pid = Environment.ProcessId,
            driver = "pawnio",
        });

        StartHeartbeat();
        var cmdThread = new Thread(() => CommandLoop(computer)) { IsBackground = true, Name = "commands" };
        cmdThread.Start();

        var exit = 0;
        try
        {
            var tick = 0;
            while (!_stop)
            {
                SnapshotDto snap;
                lock (Sync)
                {
                    snap = Poll(computer);
                }
                Interlocked.Increment(ref _polls);
                Interlocked.Exchange(ref _lastPollDoneTicks, Stopwatch.GetTimestamp());
                Emit(snap);

                if ((++tick & 1) == 0)
                {
                    var ac = AntiCheat.Detect();
                    if (ac.Count > 0)
                    {
                        Emit(new { type = "anti_cheat", processes = ac });
                        exit = 4;
                        break;
                    }
                }
                SleepInterruptible(1000);
            }
        }
        catch (Exception ex)
        {
            Emit(new { type = "error", error = "poll_failed", detail = ex.Message });
            exit = 5;
        }
        finally
        {
            _stop = true;
            SafeShutdown(computer);
        }
        return exit;
    }

    private static void SleepInterruptible(int ms)
    {
        var end = Environment.TickCount64 + ms;
        while (!_stop && Environment.TickCount64 < end) Thread.Sleep(50);
    }

    private static void StartHeartbeat()
    {
        var t = new Thread(() =>
        {
            while (!_stop)
            {
                var ageMs = (long)((Stopwatch.GetTimestamp() - Interlocked.Read(ref _lastPollDoneTicks))
                                   * 1000.0 / Stopwatch.Frequency);
                Emit(new
                {
                    type = "heartbeat",
                    seq = Interlocked.Increment(ref _seq),
                    ts_ms = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds(),
                    poll_age_ms = ageMs,
                    polls = Interlocked.Read(ref _polls),
                });
                SleepInterruptible(1000);
            }
        })
        { IsBackground = true, Name = "heartbeat" };
        t.Start();
    }

    private static void CommandLoop(Computer computer)
    {
        try
        {
            string? line;
            while (!_stop && (line = Console.ReadLine()) != null)
            {
                if (string.IsNullOrWhiteSpace(line)) continue;
                BridgeCommand? req;
                try
                {
                    req = JsonSerializer.Deserialize<BridgeCommand>(line);
                }
                catch (JsonException ex)
                {
                    // One malformed line must never kill the loop.
                    Emit(new { type = "error", error = "malformed_json", detail = ex.Message });
                    continue;
                }
                if (req?.cmd is null)
                {
                    Emit(new { type = "error", error = "missing_cmd" });
                    continue;
                }

                try
                {
                    Handle(computer, req);
                }
                catch (Exception ex)
                {
                    Emit(new { type = "ack", cmd = req.cmd, ok = false, detail = ex.Message });
                }
            }
        }
        catch (Exception)
        {
            // stdin closed or broken: treated as a stop request below
        }
        finally
        {
            _stop = true; // daemon gone: main loop restores PWM and exits
        }
    }

    private static void Handle(Computer computer, BridgeCommand req)
    {
        switch (req.cmd)
        {
            case "set_all_pwm" when req.percent is int pct:
            {
                var applied = 0;
                lock (Sync)
                {
                    foreach (var (_, ctl, kind) in EnumerateControls(computer))
                    {
                        if (kind != "chassis") continue;
                        ctl.SetSoftware(Math.Clamp(pct, ChassisFanFloorPercent, 100));
                        applied++;
                    }
                }
                Emit(new { type = "ack", cmd = req.cmd, ok = true, detail = $"chassis_controls={applied}" });
                break;
            }
            case "set_pwm" when req.percent is int pct && !string.IsNullOrEmpty(req.id):
            {
                var done = false;
                var refused = false;
                lock (Sync)
                {
                    foreach (var (sensor, ctl, kind) in EnumerateControls(computer))
                    {
                        if (!string.Equals(sensor.Identifier.ToString(), req.id, StringComparison.OrdinalIgnoreCase))
                            continue;
                        if (kind != "chassis") { refused = true; break; }
                        ctl.SetSoftware(Math.Clamp(pct, ChassisFanFloorPercent, 100));
                        done = true;
                        break;
                    }
                }
                Emit(new
                {
                    type = "ack",
                    cmd = req.cmd,
                    ok = done,
                    detail = done ? null : refused ? "refused_not_chassis" : "unknown_id",
                });
                break;
            }
            case "reset_pwm":
            {
                bool verified;
                lock (Sync) { verified = ResetAndVerify(computer); }
                Emit(new { type = "ack", cmd = req.cmd, ok = verified });
                break;
            }
            case "quit":
            case "stop":
                // Main loop performs: reset PWM first, verify, emit "stopped", close, exit.
                _stop = true;
                Emit(new { type = "ack", cmd = req.cmd, ok = true });
                break;
            default:
                Emit(new { type = "ack", cmd = req.cmd, ok = false, detail = "unknown_or_invalid_command" });
                break;
        }
    }

    /// Restores firmware control FIRST, confirms it, emits "stopped", then releases the driver.
    private static void SafeShutdown(Computer computer)
    {
        bool verified = false;
        try
        {
            lock (Sync) { verified = ResetAndVerify(computer); }
        }
        catch (Exception) { /* reported via verified=false */ }
        Emit(new { type = "stopped", pwm_reset = true, verified });
        try { computer.Close(); } catch (Exception) { }
    }

    private static bool ResetAndVerify(Computer computer)
    {
        var all = true;
        foreach (var (_, ctl, _) in EnumerateControls(computer))
        {
            try { ctl.SetDefault(); } catch (Exception) { all = false; }
        }
        // Confirm by reading the mode back (a second pass retries stragglers once).
        for (var attempt = 0; attempt < 2; attempt++)
        {
            all = true;
            foreach (var (_, ctl, _) in EnumerateControls(computer))
            {
                if (ctl.ControlMode == ControlMode.Software)
                {
                    all = false;
                    try { ctl.SetDefault(); } catch (Exception) { }
                }
            }
            if (all) break;
            Thread.Sleep(100);
        }
        return all;
    }

    // ------------------------------------------------------------------ hardware enumeration

    private static IEnumerable<(ISensor Sensor, IControl Control, string Kind)> EnumerateControls(Computer computer)
    {
        foreach (var hw in computer.Hardware.Where(h => h.HardwareType == HardwareType.Motherboard))
        {
            foreach (var sub in hw.SubHardware)
            {
                foreach (var s in sub.Sensors)
                {
                    if (s.SensorType == SensorType.Control && s.Control is not null)
                        yield return (s, s.Control, ClassifyControl(s));
                }
            }
        }
    }

    /// Never guesses "chassis": CPU_FAN / CPU_OPT and AIO pump headers are protected, generic names
    /// stay "unknown" (undriven) unless the id was passed with --chassis-id.
    internal static string ClassifyControl(ISensor s)
    {
        var name = s.Name.ToLowerInvariant();
        if (name.Contains("pump") || name.Contains("aio")) return "pump";
        if (name.Contains("cpu")) return "cpu_fan";
        if (ExtraChassisIds.Contains(s.Identifier.ToString())) return "chassis";
        if (name.Contains("system") || name.Contains("chassis") || name.Contains("case") ||
            name.Contains("sys_fan") || name.Contains("sys fan")) return "chassis";
        return "unknown";
    }

    private static double? Clean(float? v) =>
        v is float f && !float.IsNaN(f) && !float.IsInfinity(f) ? Math.Round(f, 1) : null;

    private static SnapshotDto Poll(Computer computer)
    {
        var dto = new SnapshotDto();
        var ramTemps = new List<double>();
        double? firstCpuTemp = null;

        foreach (var hw in computer.Hardware)
        {
            hw.Update();
            foreach (var sub in hw.SubHardware) sub.Update();

            if (hw.HardwareType == HardwareType.Cpu)
            {
                foreach (var s in hw.Sensors)
                {
                    var val = Clean(s.Value);
                    if (s.SensorType == SensorType.Temperature && val is double t)
                    {
                        firstCpuTemp ??= t;
                        var n = s.Name;
                        if (n.StartsWith("CCD", StringComparison.OrdinalIgnoreCase))
                        {
                            dto.cpu_ccd_temps_c.Add(new NamedValueDto { name = n, value = t });
                        }
                        else if (n.Contains("Tctl", StringComparison.OrdinalIgnoreCase))
                        {
                            dto.cpu_tctl_c ??= t;
                        }
                        else if (n.Contains("Package", StringComparison.OrdinalIgnoreCase))
                        {
                            dto.cpu_package_c ??= t;
                        }
                    }
                    else if (s.SensorType == SensorType.Power && val is double p &&
                             (s.Name.Contains("Package", StringComparison.OrdinalIgnoreCase) || dto.cpu_power_w is null))
                    {
                        dto.cpu_power_w = p;
                    }
                }
            }
            else if (hw.HardwareType == HardwareType.Motherboard)
            {
                dto.motherboard_name = hw.Name;
                foreach (var sub in hw.SubHardware)
                {
                    foreach (var s in sub.Sensors)
                    {
                        var val = Clean(s.Value);
                        if (s.SensorType == SensorType.Temperature && val is double t)
                        {
                            if (s.Name.Contains("VRM", StringComparison.OrdinalIgnoreCase) ||
                                s.Name.Contains("MOS", StringComparison.OrdinalIgnoreCase))
                                dto.motherboard_vrm_temp_c = t;
                            else if (s.Name.Contains("Chipset", StringComparison.OrdinalIgnoreCase) ||
                                     s.Name.Contains("System", StringComparison.OrdinalIgnoreCase) ||
                                     dto.motherboard_chipset_temp_c is null)
                                dto.motherboard_chipset_temp_c = t;
                        }
                        else if (s.SensorType == SensorType.Fan)
                        {
                            // Stable id = LHM sensor identifier; stopped fans (0 rpm) are kept.
                            dto.case_fans.Add(new CaseFanDto
                            {
                                id = s.Identifier.ToString(),
                                name = s.Name,
                                rpm = val ?? 0,
                            });
                        }
                        else if (s.SensorType == SensorType.Control && s.Control is not null)
                        {
                            dto.controls.Add(new ControlDto
                            {
                                id = s.Identifier.ToString(),
                                name = s.Name,
                                kind = ClassifyControl(s),
                                value_pct = val,
                                mode = s.Control.ControlMode switch
                                {
                                    ControlMode.Default => "default",
                                    ControlMode.Software => "software",
                                    _ => "undefined",
                                },
                            });
                        }
                    }
                }
            }
            else if (hw.HardwareType == HardwareType.Memory)
            {
                foreach (var s in hw.Sensors)
                {
                    if (s.SensorType == SensorType.Temperature && Clean(s.Value) is double v && v > 5 && v < 105)
                        ramTemps.Add(v);
                }
            }
        }

        dto.cpu_temp_c = dto.cpu_tctl_c ?? dto.cpu_package_c ?? firstCpuTemp;
        if (ramTemps.Count > 0) dto.ram_temp_c = Math.Round(ramTemps.Max(), 1);
        return dto;
    }

    private static bool LoadedWinRing0()
    {
        try
        {
            foreach (ProcessModule m in Process.GetCurrentProcess().Modules)
                if (m.ModuleName.Contains("winring0", StringComparison.OrdinalIgnoreCase)) return true;
        }
        catch (Exception) { }
        return false;
    }

    private static void Emit(object o)
    {
        var line = JsonSerializer.Serialize(o, o.GetType(), Json);
        lock (OutLock)
        {
            Console.Out.WriteLine(line);
            Console.Out.Flush();
        }
    }
}

/// Architecture §7 anti-cheat detection (process and service names).
internal static class AntiCheat
{
    private static readonly string[] ExactProcesses =
        { "faceitclient", "faceit", "vgc", "vgtray", "beservice", "bedaisy", "ricochet", "javelin" };

    private static readonly string[] ProcessPrefixes = { "easyanticheat" };

    private static readonly string[] Services =
        { "vgc", "vgk", "faceit", "easyanticheat", "easyanticheat_eos", "beservice", "bedaisy" };

    public static List<string> Detect()
    {
        var found = new List<string>();
        try
        {
            foreach (var p in Process.GetProcesses())
            {
                string n;
                try { n = p.ProcessName.ToLowerInvariant(); } catch (Exception) { continue; }
                if (Array.IndexOf(ExactProcesses, n) >= 0 || ProcessPrefixes.Any(n.StartsWith))
                    found.Add(n);
                p.Dispose();
            }
        }
        catch (Exception) { }
        try
        {
            foreach (var s in ServiceController.GetServices())
            {
                try
                {
                    if (Array.IndexOf(Services, s.ServiceName.ToLowerInvariant()) >= 0 &&
                        s.Status is ServiceControllerStatus.Running or ServiceControllerStatus.StartPending)
                        found.Add("service:" + s.ServiceName.ToLowerInvariant());
                }
                catch (Exception) { }
                finally { s.Dispose(); }
            }
        }
        catch (Exception) { }
        return found.Distinct().ToList();
    }
}
