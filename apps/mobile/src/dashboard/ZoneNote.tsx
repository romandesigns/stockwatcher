import { UI } from "@stockwatcher/chart-native";
import type { ZoneNoteView } from "@stockwatcher/shared";
import { StyleSheet, Text, View, type StyleProp, type ViewStyle } from "react-native";

import { Collapse } from "../motion/Collapse";
import { fonts } from "../theme";

/**
 * The volume-shelf note (shared zones.ts), the web's ZoneNote: "In volume shelf $2.41–$2.55" in
 * plain text with the research base rate muted beside it ("about 1 in 3 hold, 1 in 2 fail"),
 * then "Shelf held" / "Shelf lost" muted once the server resolves it. Information, never a call:
 * no success green, no accent, no icon. One line that truncates, so it fits the Fold's cover
 * screen; it opens and closes by its real height (Collapse), instantly with reduced motion.
 * `gap` is the parent's gap (Collapse cancels it while closed).
 */
export function ZoneNote({ note, reducedMotion, gap = 0, style }: { note: ZoneNoteView | null; reducedMotion: boolean; gap?: number; style?: StyleProp<ViewStyle> }) {
  return (
    <Collapse open={note !== null} reducedMotion={reducedMotion} gap={gap}>
      {note && (
        <View style={[styles.line, style]} accessible accessibilityLabel={note.aria} testID={`zone-${note.state}`}>
          <Text numberOfLines={1} style={[styles.text, { color: note.tone === "text" ? UI.text : UI.textDim }]}>
            {note.label}
            {note.baseRate ? <Text style={styles.rate}>{`  ${note.baseRate}`}</Text> : null}
          </Text>
        </View>
      )}
    </Collapse>
  );
}

const styles = StyleSheet.create({
  line: { flexDirection: "row", alignItems: "center" },
  text: { flexShrink: 1, fontSize: 11, fontFamily: fonts.regular, fontVariant: ["tabular-nums"] },
  rate: { color: UI.textFaint, fontFamily: fonts.regular },
});
