(ns getter.core
  (:require [getter.provider.git :as git]
            [clojure.edn :as edn]))

(defn get-app-data-folder
  []
  (let [os-name (System/getProperty "os.name")]
    (cond (.startsWith os-name "Windows") (str (System/getenv "APPDATA")
                                               "\\getter")
          (.startsWith os-name "Mac") (str
                                        (System/getProperty "user.home")
                                        "/Library/Application Support/getter")
          :else (str (System/getProperty "user.home") "/.local/share/getter"))))

(defn read-packages
  []
  (let [app-data-folder (get-app-data-folder)]
    (edn/read-string (slurp (str app-data-folder "/packages.edn")))))

(defn run
  []
  (let [packages (read-packages)]
    (doseq [package packages]
      (println "Tags for" package ":")
      (println (git/get-tags package)))))
