// The ATT&CK ids an analyst typed into a text box (an entry's techniques, an IOC's ids), split at spaces and commas.
// Splitting only: the engine checks each id and refuses what is not one.
export const attackIdsOf = (text: string): string[] => text.split(/[\s,]+/).filter((t) => t !== "");
