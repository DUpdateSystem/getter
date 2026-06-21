@getter-cli @smoke
Feature: Getter CLI repository listing
  Scenario: User lists repositories before adding any repository records
    Given an initialized getter data directory
    When I run getter repo list for that directory
    Then the command succeeds
    And the output is valid JSON
    And the output contains an empty repository list
