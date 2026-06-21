@getter-cli @smoke
Feature: Getter CLI app listing
  Scenario: User lists apps before adding any app records
    Given an initialized getter data directory
    When I run getter app list for that directory
    Then the command succeeds
    And the output is valid JSON
    And the output contains an empty app list
