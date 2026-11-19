import { createContext } from "react";
import { defaultPreferences } from "./image-source";

export const PreferencesContext = createContext(defaultPreferences);
