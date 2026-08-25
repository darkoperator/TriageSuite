using System;
using System.IO;
using System.Reflection;
using System.Resources;
using System.Collections;
using System.Collections.Generic;
using System.Text;

class Program
{
    static void Main(string[] args)
    {
        string dllPath = args.Length > 0 ? args[0] : "/Users/carlosperez/.nuget/packages/guidmapping/2026.5.0/lib/netstandard2.0/GuidMapping.dll";
        string outputPath = args.Length > 1 ? args[1] : "/tmp/guid_pairs_final.txt";
        
        var assembly = Assembly.LoadFrom(dllPath);
        var resourceNames = assembly.GetManifestResourceNames();
        
        string? targetResource = null;
        foreach (var name in resourceNames)
        {
            if (name.EndsWith(".resources"))
            {
                targetResource = name;
                break;
            }
        }
        
        if (targetResource == null)
        {
            Console.Error.WriteLine("ERROR: No .resources found!");
            Environment.Exit(1);
        }
        
        Console.Error.WriteLine($"Using resource: {targetResource}");
        
        using var stream = assembly.GetManifestResourceStream(targetResource)!;
        var reader = new ResourceReader(stream);
        
        // Find the GuidToName key
        string? rawValue = null;
        foreach (DictionaryEntry entry in reader)
        {
            if (entry.Key?.ToString() == "GuidToName" && entry.Value is string s)
            {
                rawValue = s;
                break;
            }
        }
        
        if (rawValue == null)
        {
            Console.Error.WriteLine("ERROR: GuidToName key not found!");
            Environment.Exit(1);
        }
        
        // Split on CRLF lines — these are the raw .NET string characters, fully decoded
        var lines = rawValue.Split(new[] { "\r\n", "\n", "\r" }, StringSplitOptions.None);
        Console.Error.WriteLine($"Total lines (including trailing empty): {lines.Length}");
        
        var pairs = new List<(string guid, string name)>();
        int skipped = 0;
        
        foreach (var line in lines)
        {
            if (string.IsNullOrEmpty(line)) continue;
            
            int pipeIdx = line.IndexOf('|');
            if (pipeIdx < 0)
            {
                Console.Error.WriteLine($"WARNING: no pipe in line: {line}");
                skipped++;
                continue;
            }
            
            string guid = line.Substring(0, pipeIdx);
            string name = line.Substring(pipeIdx + 1);
            // Do NOT trim — preserve the exact name as stored in the resource
            pairs.Add((guid, name));
        }
        
        Console.Error.WriteLine($"Pairs found: {pairs.Count}, skipped: {skipped}");
        
        // Sort by GUID ascending (binary_search requires sorted order)
        pairs.Sort((a, b) => string.Compare(a.guid, b.guid, StringComparison.Ordinal));
        
        // Write output as UTF-8 without BOM
        using var sw = new StreamWriter(outputPath, false, new UTF8Encoding(false));
        foreach (var (guid, name) in pairs)
        {
            sw.WriteLine($"{guid}|{name}");
        }
        
        Console.Error.WriteLine($"Done. Written {pairs.Count} entries to {outputPath}");
        
        // Spot-check known-previously-defective entries
        string[] spotCheckGuids = {
            "cafeefac-0015-0000-0007-abcdeffedcbc",  // Java Plug-in 1.5.0_07 (was missing)
            "113fae2b-ff38-4054-9229-467410d6eb27",  // Intel® AAC encoder (was truncated)
            "056d528d-ce28-4194-9ba3-ba2e9197ff8c",  // \x01 MEGA (Pending) (was emptied)
            "15eae92e-f17a-4431-9f28-805e482dafd4",  // "Install New Programs " (trailing space)
            "6da1ed92-315e-4d0b-b354-9d5f519dba95",  // "   " (spaces only)
        };

        Console.Error.WriteLine("\n=== SPOT CHECKS ===");
        foreach (var g in spotCheckGuids)
        {
            var match = pairs.Find(p => p.guid == g);
            if (match.guid != null)
                Console.Error.WriteLine($"  FOUND {g} -> '{match.name}' (len={match.name.Length})");
            else
                Console.Error.WriteLine($"  MISSING {g}");
        }

        // Check for any control chars in names
        int ctrlCount = 0;
        foreach (var (guid, name) in pairs)
        {
            foreach (char c in name)
            {
                if (c < 32 && c != '\t')
                {
                    Console.Error.WriteLine($"  CTRL in {guid}: name has char 0x{(int)c:X2} at name='{name}'");
                    ctrlCount++;
                    break;
                }
            }
        }
        Console.Error.WriteLine($"Entries with control chars in name: {ctrlCount}");
    }
}
