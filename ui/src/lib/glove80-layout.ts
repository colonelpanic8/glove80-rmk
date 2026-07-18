export type KeySpec = {
  logicalIndex: number;
  ledIndex: number;
  mirrorLedIndex: number;
  label: string;
  x: number;
  y: number;
  kind: "main" | "thumb";
};

const LABELS = [
  "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10",
  "=", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "−",
  "Tab", "Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P", "\\",
  "Ctrl", "A", "S", "D", "F", "G", "H", "J", "K", "L", ";", "'",
  "Shift", "Z", "X", "C", "V", "B", "Esc", "Del", "Magic", "⌘", "L4", "⌫",
  "N", "M", ",", ".", "/", "Shift", "`", "Home", "End", "←", "→", "⌫",
  "⌘", "Alt", "⌘", "Enter", "Space", "↑", "↓", "[", "]", "L3",
] as const;

const MAIN_LED_ROWS_LEFT = [
  [34, 28, 22, 16, 10],
  [35, 29, 23, 17, 11, 6],
  [36, 30, 24, 18, 12, 7],
  [37, 31, 25, 19, 13, 8],
  [38, 32, 26, 20, 14, 9],
  [39, 33, 27, 21, 15],
] as const;

const LOGICAL_ROWS = [
  { left: [0, 1, 2, 3, 4], right: [5, 6, 7, 8, 9] },
  { left: [10, 11, 12, 13, 14, 15], right: [16, 17, 18, 19, 20, 21] },
  { left: [22, 23, 24, 25, 26, 27], right: [28, 29, 30, 31, 32, 33] },
  { left: [34, 35, 36, 37, 38, 39], right: [40, 41, 42, 43, 44, 45] },
  { left: [46, 47, 48, 49, 50, 51], right: [58, 59, 60, 61, 62, 63] },
  { left: [64, 65, 66, 67, 68], right: [75, 76, 77, 78, 79] },
] as const;

function key(logicalIndex: number, ledIndex: number, mirrorLedIndex: number, x: number, y: number, kind: KeySpec["kind"]): KeySpec {
  return { logicalIndex, ledIndex, mirrorLedIndex, label: LABELS[logicalIndex], x, y, kind };
}

function buildLayout(): KeySpec[] {
  const keys: KeySpec[] = [];
  for (let row = 0; row < MAIN_LED_ROWS_LEFT.length; row++) {
    const leftLeds = MAIN_LED_ROWS_LEFT[row];
    const rightLeds = [...leftLeds].reverse().map((index) => index + 40);
    const leftLogical = LOGICAL_ROWS[row].left;
    const rightLogical = LOGICAL_ROWS[row].right;
    const leftOffset = leftLeds.length === 5 ? 0.45 : 0;
    const rightOffset = rightLeds.length === 5 ? 0.45 : 0;
    leftLeds.forEach((ledIndex, column) => {
      keys.push(key(leftLogical[column], ledIndex, rightLeds[column], leftOffset + column, row, "main"));
    });
    rightLeds.forEach((ledIndex, column) => {
      keys.push(key(rightLogical[column], ledIndex, leftLeds[column], 14.6 + rightOffset + column, row, "main"));
    });
  }

  const leftThumbLogical = [52, 53, 54, 69, 70, 71];
  const rightThumbLogical = [55, 56, 57, 72, 73, 74];
  const leftThumbLeds = [0, 1, 2, 3, 4, 5];
  const rightThumbLeds = [42, 41, 40, 45, 44, 43];
  for (let index = 0; index < 6; index++) {
    const column = index % 3;
    const row = Math.floor(index / 3);
    keys.push(key(leftThumbLogical[index], leftThumbLeds[index], rightThumbLeds[index], 5.6 + column, 4.05 + row, "thumb"));
    keys.push(key(rightThumbLogical[index], rightThumbLeds[index], leftThumbLeds[index], 11.0 + column, 4.05 + row, "thumb"));
  }
  return keys.sort((a, b) => a.logicalIndex - b.logicalIndex);
}

export const GLOVE80_KEYS = buildLayout();

export function colorsByLed(keys: readonly KeySpec[], colorsByLogicalKey: readonly number[]): number[] {
  const result = Array<number>(80).fill(0);
  for (const keySpec of keys) result[keySpec.ledIndex] = colorsByLogicalKey[keySpec.logicalIndex] ?? 0;
  return result;
}

export function mirrorLeftToRight(colorsByLogicalKey: readonly number[]): number[] {
  const result = [...colorsByLogicalKey];
  const logicalByLed = new Map(GLOVE80_KEYS.map((item) => [item.ledIndex, item.logicalIndex]));
  for (const keySpec of GLOVE80_KEYS) {
    if (keySpec.ledIndex >= 40) continue;
    const target = logicalByLed.get(keySpec.mirrorLedIndex);
    if (target !== undefined) result[target] = result[keySpec.logicalIndex];
  }
  return result;
}
