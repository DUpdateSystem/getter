(ns getter.provider.zypper
  (:require [clojure.java.shell :as shell :refer [sh]]
            [clojure.string :as str]))

(defn parse-header
  [header-line]
  (let [columns (str/split header-line #"\|")
        name-index (->> columns
                        (map-indexed vector)
                        (filter #(str/includes? (second %) "Name"))
                        first
                        first)
        version-index (->> columns
                           (map-indexed vector)
                           (filter #(str/includes? (second %) "Version"))
                           first
                           first)]
    {:name-index name-index, :version-index version-index}))

(defn parse-data
  [header-info lines]
  (let [name-index (:name-index header-info)
        version-index (:version-index header-info)]
    (loop [result {}
           remaining-lines lines]
      (if (empty? remaining-lines)
        result
        (let [line (first remaining-lines)
              columns (str/split line #"\|")
              package-name (str/trim (nth columns name-index))
              version (str/trim (nth columns version-index))]
          (recur (assoc result package-name version)
                 (rest remaining-lines)))))))

(defn find-header-line
  [lines]
  (some (fn [[idx line]]
          (when (and (re-find #"^S\s+\|" line)
                     (< (inc idx) (count lines))
                     (re-find #"^\-+\+" (nth lines (inc idx))))
            idx))
        (map vector (range) lines)))

(defn get-installed-versions
  [& package-names]
  (let [zypper-output (shell/with-sh-env {:LANG "en_US.UTF-8"}
                                         (apply sh
                                           "zypper" "search"
                                           "-s" "--installed-only"
                                           "--match-exact" package-names))
        zypper-output-lines (->> (:out zypper-output)
                                 str/split-lines)
        header-line-index (find-header-line zypper-output-lines)]
    (if (nil? header-line-index)
      (into {} (map #(hash-map % nil) package-names))
      (let [header-line (nth zypper-output-lines header-line-index)
            header-info (parse-header header-line)
            data-lines (subvec zypper-output-lines (+ 2 header-line-index))
            parsed-data (parse-data header-info data-lines)]
        (merge (into {} (map #(hash-map % nil) package-names)) parsed-data)))))
