import { useState } from "react";
import { createRoot } from "react-dom/client";
import MorphText from "../../src/components/ui/MorphText";

function Fixture() {
  const [value, setValue] = useState("Indexing");
  return (
    <>
      <button onClick={() => setValue(value === "Ready" ? "Indexing" : "Ready")}>
        Change value
      </button>
      <button data-testid="value">
        <MorphText>{value}</MorphText>
      </button>
    </>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
