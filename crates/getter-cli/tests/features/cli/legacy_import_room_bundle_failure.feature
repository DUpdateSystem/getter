@getter-cli @migration
Feature: Legacy import failure recovery
  Scenario: User receives a non-destructive report when legacy import fails
    Given an initialized getter data directory
    And a corrupted legacy export bundle
    When I run getter legacy import-room-bundle for that bundle
    Then the command fails with a documented migration error
    And no partially usable imported state is created
    And a sanitized migration report is available

  Scenario: User imports a valid legacy bundle into tracked app state
    Given an initialized getter data directory
    And a syntactically valid legacy export bundle with an Android app
    When I run getter legacy import-room-bundle for that bundle
    Then the command succeeds
    And the output is valid JSON
    And the import reports one tracked app
    And the app list contains imported package "android/org.fdroid.fdroid"
