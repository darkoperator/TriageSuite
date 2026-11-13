// DEV AND TEST ONLY: the mock backend's data. Seeded, so every run (and every test) sees the same rows. All names are
// synthetic (WS01, SRV01, acme.local, 203.0.113.0/24, 198.51.100.0/24, 10.0.0.0/8). No `Date` is used: the front end
// does no date arithmetic, so the times are written by integer arithmetic over a fixed day in January 2026.
import type { ColumnMeta } from "../contract/generated/ColumnMeta";

type Cells = ReadonlyArray<ReadonlyArray<string | null>>;

/** The values every screen must show as inert text, copy and export byte for byte (spec: Review Focus 1). */
export const HOSTILE = {
  html: "<img src=x onerror=alert(1)>",
  bidi: "report\u202Egpj.exe",
  joiner: "a\u200Db",
  bell: "ring\u0007ring",
  formula: '=HYPERLINK("http://x")',
  long: ("LONG-" + "0123456789abcdef".repeat(400)).slice(0, 5000),
} as const;

export const EVENT_ROWS = 5000;
export const HAYABUSA_ROWS = 300;
/** Row ids are the case's: the events take 1..5000, the Hayabusa rows follow. */
export const HAYABUSA_FIRST_ID = EVENT_ROWS + 1;

const RUN_ID = "20260101000000-aaaa";
const RUN_DIR = "/runs/run-20260101000000-aaaa/Processed-WS01-20260101000000";

const col = (name: string, over: Partial<ColumnMeta> = {}): ColumnMeta => ({
  name, role: "data", sql_type: null, time_semantics: null, origin: null, ...over,
});
const bigint = (name: string) => col(name, { sql_type: "BIGINT", origin: "suite" });

const TRIAGE_COLUMNS: ColumnMeta[] = [
  col("_triage_run_id", { role: "run_id" }),
  col("_triage_host", { role: "host" }),
  col("_triage_identity", { role: "identity" }),
  col("_triage_output_file", { role: "output_file" }),
];

/** 40 columns: the four `_triage_*` ones, a utc time, a time of unknown zone, numbers, and a lot of text. */
export const EVENT_COLUMNS: ColumnMeta[] = [
  ...TRIAGE_COLUMNS,
  col("TimeCreated", { sql_type: "TIMESTAMP", time_semantics: "utc", origin: "suite" }),
  col("LocalTime", { sql_type: "TIMESTAMP", time_semantics: "unknown", origin: "sniffed" }),
  bigint("EventId"), bigint("RecordId"),
  col("Channel"), col("Provider"), col("Level"), col("Computer"),
  col("UserName"), col("SubjectUser"), col("TargetUser"), col("LogonType"),
  col("RemoteHost"), col("SourceIp"), bigint("SourcePort"),
  col("ProcessName"), bigint("ProcessId"), col("ParentProcess"), col("CommandLine"), col("ParentCommandLine"),
  col("ImagePath"), col("Hash"), col("MapDescription"),
  col("PayloadData1"), col("PayloadData2"), col("PayloadData3"), col("PayloadData4"),
  col("Message"), col("Keywords"), col("Task"), col("Opcode"), col("Version"), col("ThreadId"), col("Category"),
  col("Sid"), col("SourceFile"),
];

export const HAYABUSA_COLUMNS: ColumnMeta[] = [
  ...TRIAGE_COLUMNS,
  col("Timestamp", { sql_type: "TIMESTAMP", time_semantics: "utc", origin: "suite" }),
  col("Level"), col("RuleTitle"), col("Details"), col("Computer"), bigint("EventID"), col("Channel"), col("RuleFile"),
];

/** mulberry32: a small seeded generator. */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const pick = <T>(r: () => number, items: readonly T[]): T => items[Math.floor(r() * items.length)];
const pad = (n: number, width = 2) => String(n).padStart(width, "0");

/** `2026-01-05T02:13:58.2040000Z` for second `s` of the day, the engine's way of writing a utc time. */
function utcText(s: number, fraction: number): string {
  const day = 5 + Math.floor(s / 86400);
  const rest = s % 86400;
  return `2026-01-${pad(day)}T${pad(Math.floor(rest / 3600))}:${pad(Math.floor(rest / 60) % 60)}:${pad(rest % 60)}.${pad(fraction, 7)}Z`;
}

/** The same instant as a file without a zone writes it (`2026-01-05 02:13:58`). */
const localText = (s: number) => utcText(s, 0).slice(0, 19).replace("T", " ");

const CHANNELS = ["Security", "Security", "Security", "System", "Application", "Microsoft-Windows-Sysmon/Operational", "Microsoft-Windows-PowerShell/Operational"] as const;
const PROVIDERS = ["Microsoft-Windows-Security-Auditing", "Service Control Manager", "Microsoft-Windows-Sysmon", "PowerShell"] as const;
const EVENT_IDS = [4624, 4624, 4625, 4634, 4648, 4672, 4688, 4697, 7045, 1, 3, 11, 4104] as const;
const MAP = ["Successful logon", "Failed logon", "Logoff", "Process created", "Service installed", "Network connection", "File created", "Script block"] as const;
const USERS = ["administrator", "svc_backup", "jdoe", "asmith", "SYSTEM", "WS01$", "SRV01$"] as const;
const PROCS = ["C:\\Windows\\System32\\svchost.exe", "C:\\Windows\\System32\\cmd.exe", "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", "C:\\Users\\jdoe\\AppData\\Local\\Temp\\setup.exe", "C:\\Program Files\\Acme\\agent.exe"] as const;
const LEVELS = ["Information", "Information", "Warning", "Error"] as const;

let events: Cells | null = null;

/** The 5,000 `mock_events` rows (row id = index + 1), each 40 cells wide, in `EVENT_COLUMNS` order. */
export function eventRows(): Cells {
  if (events) return events;
  const r = rng(20260105);
  const rows: (string | null)[][] = [];
  for (let i = 0; i < EVENT_ROWS; i++) {
    const s = 2 * 3600 + 13 * 60 + 58 + i * 17 + Math.floor(r() * 12);
    const host = r() < 0.7 ? "WS01" : "SRV01";
    const channel = pick(r, CHANNELS);
    const eventId = pick(r, EVENT_IDS);
    const proc = pick(r, PROCS);
    const remote = r() < 0.4 ? `203.0.113.${1 + Math.floor(r() * 250)}` : `10.0.${Math.floor(r() * 4)}.${1 + Math.floor(r() * 250)}`;
    const sid = `S-1-5-21-1000000000-2000000000-3000000000-${1000 + Math.floor(r() * 8)}`;
    // The hostile values land in the first rows (so they are on the first screen) and then every 997th row.
    const hostile = i % 997;
    const row: (string | null)[] = [
      RUN_ID, host, null, `${RUN_DIR}/EventLogs/evtx.csv`,
      utcText(s, Math.floor(r() * 10_000_000)), localText(s),
      String(eventId), String(100000 + i),
      channel, pick(r, PROVIDERS), pick(r, LEVELS), host === "WS01" ? "WS01.acme.local" : "SRV01.acme.local",
      pick(r, USERS), pick(r, USERS), pick(r, USERS), String(pick(r, [2, 3, 5, 10] as const)),
      remote, remote, String(49152 + Math.floor(r() * 16000)),
      proc, String(400 + Math.floor(r() * 9000)), pick(r, PROCS), `${proc} -k netsvcs -p`, `${pick(r, PROCS)} /c start`,
      proc, r().toString(16).slice(2, 18).padEnd(16, "0") + r().toString(16).slice(2, 18).padEnd(16, "0"),
      pick(r, MAP),
      `LogonType ${pick(r, [2, 3, 5, 10] as const)}`, `Account ${pick(r, USERS)}`, `Host ${host}`, `Source ${remote}`,
      `${pick(r, MAP)} on ${host} at ${localText(s)}`,
      pick(r, ["Audit Success", "Audit Failure", "Classic"] as const), String(pick(r, [12544, 13312, 0] as const)), String(pick(r, [0, 1, 2] as const)),
      String(pick(r, [0, 1, 2] as const)), String(1000 + Math.floor(r() * 9000)), pick(r, ["Logon", "Process Creation", "Network", "None"] as const),
      sid, "C:\\Windows\\System32\\winevt\\Logs\\" + channel.split("/")[0].replace("Microsoft-Windows-", "") + ".evtx",
    ];
    const set = (name: string, value: string) => {
      row[EVENT_COLUMNS.findIndex((c) => c.name === name)] = value;
    };
    if (hostile === 1) set("UserName", HOSTILE.html);
    if (hostile === 2) set("UserName", HOSTILE.bidi);
    if (hostile === 3) set("ProcessName", HOSTILE.joiner);
    if (hostile === 4) set("Message", HOSTILE.bell);
    if (hostile === 5) set("CommandLine", HOSTILE.formula);
    if (hostile === 6) set("Message", HOSTILE.long);
    rows.push(row);
  }
  events = rows;
  return rows;
}

let hayabusa: Cells | null = null;

const RULES = [
  ["crit", "Mimikatz-like credential dump", "Process: lsass.exe access by C:\\Users\\jdoe\\AppData\\Local\\Temp\\setup.exe"],
  ["high", "Failed logon from external address", "Logon type 10 from 203.0.113.7 to WS01"],
  ["med", "Service installed in a user path", "Service: AcmeAgent  Image: C:\\Users\\jdoe\\AppData\\Local\\Temp\\setup.exe"],
  ["low", "Scheduled task created", "Task: \\Acme\\Update  Author: SRV01$"],
  ["info", "Logon with explicit credentials", "Account: svc_backup  Target: SRV01.acme.local"],
] as const;

/** The 300 `mock_hayabusa` rows (row ids follow the events'), in `HAYABUSA_COLUMNS` order. */
export function hayabusaRows(): Cells {
  if (hayabusa) return hayabusa;
  const r = rng(7);
  const rows: (string | null)[][] = [];
  for (let i = 0; i < HAYABUSA_ROWS; i++) {
    const s = 2 * 3600 + 14 * 60 + i * 281 + Math.floor(r() * 60);
    const [level, title, details] = pick(r, RULES);
    rows.push([
      RUN_ID, "WS01", null, `${RUN_DIR}/Hayabusa/timeline.csv`,
      utcText(s, 0), level, title, details, "WS01.acme.local",
      String(pick(r, [4624, 4625, 4688, 7045, 4698] as const)), pick(r, ["Sec", "Sys", "PwSh"] as const),
      `rules/sigma/${title.toLowerCase().replace(/[^a-z]+/g, "_")}.yml`,
    ]);
  }
  hayabusa = rows;
  return rows;
}
