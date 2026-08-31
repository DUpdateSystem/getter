@getter-cli @migration
Feature: Direct legacy Room database import
  Scenario: User imports a supported legacy Room database into tracked app state
    Given an initialized getter data directory
    And a legacy Room v17 database with an Android app and extra app state
    When I run getter legacy import-room-db for that database
    Then the command succeeds
    And the output is valid JSON
    And the import reports one tracked app
    And the direct migration reports dropped legacy hub warnings
    And the direct migration report stays sanitized
    And the app list contains directly imported package "android/org.fdroid.fdroid"
    When I run getter legacy report-list for that directory
    Then the output lists migration report "migration.imported"
    And the direct migration report list stays sanitized

  Scenario: User does not rerun a completed direct legacy Room migration
    Given an initialized getter data directory
    And a legacy Room v17 database with an Android app and extra app state
    When I run getter legacy import-room-db for that database
    Then the command succeeds
    When I run getter legacy import-room-db for that database
    Then the command succeeds
    And the output reports the legacy Room migration was already completed

  Scenario: User cannot apply a bridge bundle after completed direct legacy Room migration
    Given an initialized getter data directory
    And a legacy Room v17 database with an Android app and extra app state
    And a syntactically valid legacy export bundle with an Android app
    When I run getter legacy import-room-db for that database
    Then the command succeeds
    When I run getter legacy import-room-bundle for that bundle
    Then the command succeeds
    And the bundle output reports the legacy Room migration was already completed
    And the app list contains directly imported package "android/org.fdroid.fdroid"

  Scenario: User cannot apply a direct legacy Room database after completed bridge bundle migration
    Given an initialized getter data directory
    And a syntactically valid legacy export bundle with an Android app
    And a legacy Room v17 database with an Android app and extra app state
    When I run getter legacy import-room-bundle for that bundle
    Then the command succeeds
    When I run getter legacy import-room-db for that database
    Then the command succeeds
    And the output reports the legacy Room migration was already completed
    And the app list contains imported package "android/org.fdroid.fdroid"

  Scenario: User receives a recovery report for an unsupported legacy Room database
    Given an initialized getter data directory
    And an unsupported legacy Room database
    When I run getter legacy import-room-db for that database
    Then the command fails with direct DB migration error "migration.unsupported_db"
    And no partially usable imported state is created
    And a sanitized migration report is available

  Scenario: User receives a recovery report for a malformed legacy Room database
    Given an initialized getter data directory
    And a malformed legacy Room database
    When I run getter legacy import-room-db for that database
    Then the command fails with direct DB migration error "migration.invalid_db"
    And no partially usable imported state is created
    And a sanitized migration report is available

  Scenario: User receives a recovery report when no legacy app rows can be mapped
    Given an initialized getter data directory
    And a legacy Room v17 database with only unsupported app rows
    When I run getter legacy import-room-db for that database
    Then the command fails with direct DB migration error "migration.invalid_db"
    And no partially usable imported state is created
    And a sanitized migration report is available
