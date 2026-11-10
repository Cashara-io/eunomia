import { connectionsLayout, type Scene } from "./connections-model";
self.onmessage = (event: MessageEvent<{ scene: Scene; focus: string }>) => {
  self.postMessage(connectionsLayout(event.data.scene, event.data.focus));
};
