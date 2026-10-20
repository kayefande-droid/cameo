import { useState } from "react";
import { deleteTask } from "../services/store";

export default function DeleteConfirm({
  taskId,
  onDeleted,
}: {
  taskId: string;
  onDeleted: () => void;
}) {
  const [confirming, setConfirming] = useState(false);

  if (!confirming) {
    return (
      <button className="btn small danger" onClick={() => setConfirming(true)}>
        Delete task
      </button>
    );
  }
  return (
    <span style={{ display: "inline-flex", gap: 8, alignItems: "center" }}>
      <span className="note danger">Delete this task and its events?</span>
      <button
        className="btn small danger"
        onClick={async () => {
          await deleteTask(taskId);
          setConfirming(false);
          onDeleted();
        }}
      >
        Yes, delete
      </button>
      <button className="btn small" onClick={() => setConfirming(false)}>
        Keep
      </button>
    </span>
  );
}
