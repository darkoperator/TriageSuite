// The Overview tab until step 4: what the case holds, one line per relation with its rows and hosts.
import { displaySafe } from "../text";
import { useShell } from "./context";

export default function Overview() {
  const { summary, openDataset } = useShell();
  return (
    <div className="overview">
      <h2>{displaySafe(summary.name)}</h2>
      <table className="overview-table">
        <thead>
          <tr>
            <th scope="col">Relation</th>
            <th scope="col" className="num">Rows</th>
            <th scope="col">Hosts</th>
          </tr>
        </thead>
        <tbody>
          {summary.relations.map((r) => (
            <tr key={r.name}>
              <td>
                <button type="button" className="link" onClick={() => openDataset(r.name)}>
                  {displaySafe(r.name)}
                </button>
              </td>
              <td className="num">{r.rows}</td>
              <td>{r.hosts.map(displaySafe).join(", ")}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
