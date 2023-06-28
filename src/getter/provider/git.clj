(ns getter.provider.git
  (:require [clojure.java.shell :refer [sh]]
            [clojure.string :as str]))

(defn get-latest-hash
  [repo]
  (let [cmd (list "git" "ls-remote" repo "HEAD")]
    (->> (apply sh cmd)
         (:out)
         (re-find #"\w+"))))

(defn get-tags
  [repo]
  (let [cmd (list "git" "ls-remote" "--tags" repo)]
    (->> (apply sh cmd)
         (:out)
         (str/split-lines)
         (map #(str/split % #"\s+"))
         (map (fn [[hash tag]] {(str/replace tag #"refs/tags/" "") hash}))
         (into {}))))
