using System.Text.Json;
using LibreHardwareMonitor.Hardware;

namespace Ctrlmate.LhmBridge;

internal sealed class BridgeCommand
{
    public string? cmd { get; set; }
    public int? percent { get; set; }
}

internal sealed class CaseFanDto
{
    public string id { get; set; } = "";
    public string name { get; set; } = "";
    public double rpm { get; set; }
    public double? duty_pct { get; set; }
}

internal sealed class SnapshotDto
{
    public double? cpu_temp_c { get; set; }
    public double? cpu_power_w { get; set; }
    public string? motherboard_name { get; set; }
    public double? motherboard_vrm_temp_c { get; set; }
    public double? motherboard_chipset_temp_c { get; set; }
    public double? ram_temp_c { get; set; }
    public List<CaseFanDto> case_fans { get; set; } = new();
}

internal static class Program
{
    private static readonly object Sync = new();
    private static volatile bool _running = true;

    public static void Main()
    {
        var computer = new Computer
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

        // Stdin command reader (PWM control & clean BIOS restore on exit)
        var cmdThread = new Thread(() =>
        {
            try
            {
                string? line;
                while ((line = Console.ReadLine()) != null)
                {
                    var req = JsonSerializer.Deserialize<BridgeCommand>(line);
                    if (req?.cmd is null) continue;

                    lock (Sync)
                    {
                        switch (req.cmd)
                        {
                            case "set_all_pwm" when req.percent is int pct:
                                ApplyMotherboardPwm(computer, Math.Clamp(pct, 25, 100));
                                break;
                            case "reset_pwm":
                                ResetMotherboardPwm(computer);
                                break;
                            case "quit":
                                ResetMotherboardPwm(computer);
                                _running = false;
                                return;
                        }
                    }
                }
            }
            catch
            {
                // Stdin closed by parent daemon
            }
            finally
            {
                lock (Sync)
                {
                    ResetMotherboardPwm(computer);
                }
                _running = false;
            }
        })
        {
            IsBackground = true
        };
        cmdThread.Start();

        try
        {
            while (_running)
            {
                SnapshotDto snap;
                lock (Sync)
                {
                    snap = Poll(computer);
                }
                Console.WriteLine(JsonSerializer.Serialize(snap));
                Console.Out.Flush();
                Thread.Sleep(1000);
            }
        }
        finally
        {
            lock (Sync)
            {
                ResetMotherboardPwm(computer);
            }
            computer.Close();
        }
    }

    private static SnapshotDto Poll(Computer computer)
    {
        var dto = new SnapshotDto();
        var ramTemps = new List<double>();

        foreach (var hw in computer.Hardware)
        {
            hw.Update();
            foreach (var sub in hw.SubHardware)
            {
                sub.Update();
            }

            if (hw.HardwareType == HardwareType.Cpu)
            {
                foreach (var s in hw.Sensors)
                {
                    if (s.Value is not float val || float.IsNaN(val)) continue;
                    if (s.SensorType == SensorType.Temperature &&
                        (s.Name.Contains("Package", StringComparison.OrdinalIgnoreCase) ||
                         s.Name.Contains("Tctl", StringComparison.OrdinalIgnoreCase) ||
                         dto.cpu_temp_c is null))
                    {
                        dto.cpu_temp_c = Math.Round(val, 1);
                    }
                    else if (s.SensorType == SensorType.Power &&
                             (s.Name.Contains("Package", StringComparison.OrdinalIgnoreCase) ||
                              dto.cpu_power_w is null))
                    {
                        dto.cpu_power_w = Math.Round(val, 1);
                    }
                }
            }
            else if (hw.HardwareType == HardwareType.Motherboard)
            {
                dto.motherboard_name = hw.Name;
                var fanIdx = 0;
                foreach (var sub in hw.SubHardware)
                {
                    foreach (var s in sub.Sensors)
                    {
                        if (s.Value is not float val || float.IsNaN(val)) continue;
                        if (s.SensorType == SensorType.Temperature)
                        {
                            if (s.Name.Contains("VRM", StringComparison.OrdinalIgnoreCase) ||
                                s.Name.Contains("MOS", StringComparison.OrdinalIgnoreCase))
                            {
                                dto.motherboard_vrm_temp_c = Math.Round(val, 1);
                            }
                            else if (s.Name.Contains("Chipset", StringComparison.OrdinalIgnoreCase) ||
                                     s.Name.Contains("System", StringComparison.OrdinalIgnoreCase) ||
                                     dto.motherboard_chipset_temp_c is null)
                            {
                                dto.motherboard_chipset_temp_c = Math.Round(val, 1);
                            }
                        }
                        else if (s.SensorType == SensorType.Fan && val > 50)
                        {
                            dto.case_fans.Add(new CaseFanDto
                            {
                                id = $"mb_fan_{fanIdx++}",
                                name = s.Name,
                                rpm = Math.Round(val, 0),
                            });
                        }
                    }
                }
            }
            else if (hw.HardwareType == HardwareType.Memory)
            {
                foreach (var s in hw.Sensors)
                {
                    if (s.SensorType == SensorType.Temperature && s.Value is float val && val > 5 && val < 105)
                    {
                        ramTemps.Add(val);
                    }
                }
            }
        }

        if (ramTemps.Count > 0)
        {
            dto.ram_temp_c = Math.Round(ramTemps.Max(), 1);
        }

        return dto;
    }

    private static void ApplyMotherboardPwm(Computer computer, int percent)
    {
        foreach (var hw in computer.Hardware.Where(h => h.HardwareType == HardwareType.Motherboard))
        {
            foreach (var sub in hw.SubHardware)
            {
                foreach (var s in sub.Sensors.Where(x => x.SensorType == SensorType.Control && x.Control is not null))
                {
                    s.Control!.SetSoftware(percent);
                }
            }
        }
    }

    private static void ResetMotherboardPwm(Computer computer)
    {
        foreach (var hw in computer.Hardware.Where(h => h.HardwareType == HardwareType.Motherboard))
        {
            foreach (var sub in hw.SubHardware)
            {
                foreach (var s in sub.Sensors.Where(x => x.SensorType == SensorType.Control && x.Control is not null))
                {
                    s.Control!.SetDefault();
                }
            }
        }
    }
}
