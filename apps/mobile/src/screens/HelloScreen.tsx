import { StyleSheet, Text, View } from "react-native";

import { colors, type } from "../theme";

export function HelloScreen() {
  return (
    <View style={styles.container}>
      <Text style={styles.text}>Hello world</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    backgroundColor: colors.bg,
  },
  text: {
    ...type.title,
    color: colors.text,
  },
});
