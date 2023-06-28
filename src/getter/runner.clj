(ns getter.runner
  (:require [clojure.java.io :as io]
            [clojure.edn :as edn]
            [clojure.pprint :as pprint]
            [getter.core :as getter]))

(defn ensure-packages-file
  []
  (let [app-data-folder (io/file (getter/get-app-data-folder))
        packages-file (io/file app-data-folder "packages.edn")]
    (when-not (.exists app-data-folder) (.mkdir app-data-folder))
    (when-not (.exists packages-file)
      (with-open [w (clojure.java.io/writer packages-file)]
        (pprint/pprint '() w)))))

(defn run [& args] (ensure-packages-file) (getter/run))
