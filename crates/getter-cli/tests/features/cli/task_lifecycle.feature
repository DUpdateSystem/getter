@getter-cli @task
Feature: Offline task lifecycle
  Scenario: User submits and lists an offline fake download task
    Given an initialized getter data directory
    And an offline download request for package "android/org.fdroid.fdroid"
    When I run getter task submit for that request
    Then the command succeeds
    And the output is valid JSON
    And I remember the submitted task id
    When I run getter task list
    Then the command succeeds
    And the task list contains the remembered task with status "queued"

  Scenario: User cancels a queued offline task
    Given an initialized getter data directory
    And an offline download request for package "android/org.fdroid.fdroid"
    When I run getter task submit for that request
    Then the command succeeds
    And I remember the submitted task id
    When I run getter task cancel for the remembered task
    Then the command succeeds
    And the task cancel result has status "canceled" and changed true
    When I run getter task cancel for the remembered task
    Then the command succeeds
    And the task cancel result has status "canceled" and changed false

  Scenario: User cannot cancel a succeeded offline task
    Given an initialized getter data directory
    And an offline download request for package "android/org.fdroid.fdroid"
    When I run getter task submit for that request
    Then the command succeeds
    And I remember the submitted task id
    When I run getter task run for the remembered task
    Then the command succeeds
    And the task run result has status "succeeded" and install handoff "requested"
    When I run getter task cancel for the remembered task
    Then the command fails with a download task error

  Scenario: User polls offline task events with cursor and limit
    Given an initialized getter data directory
    And an offline download request for package "android/org.fdroid.fdroid"
    When I run getter task submit for that request
    Then the command succeeds
    And I remember the submitted task id
    When I run getter task run for the remembered task
    Then the command succeeds
    When I run getter task events after 0 limit 2
    Then the command succeeds
    And the task events output contains 2 events and has more events
    And I remember the next event cursor
    When I run getter task events after the remembered cursor limit 10
    Then the command succeeds
    And the task events output contains event "install_handoff_requested"

  Scenario: User records an offline install handoff result
    Given an initialized getter data directory
    And an offline download request for package "android/org.fdroid.fdroid"
    When I run getter task submit for that request
    Then the command succeeds
    And I remember the submitted task id
    When I run getter task run for the remembered task
    Then the command succeeds
    And I remember the install handoff id
    When I run getter task install-result "succeeded" for the remembered handoff
    Then the command succeeds
    And the install result output has status "succeeded"

  Scenario: User cannot record getter-created requested state as an install result
    Given an initialized getter data directory
    And an offline download request for package "android/org.fdroid.fdroid"
    When I run getter task submit for that request
    Then the command succeeds
    And I remember the submitted task id
    When I run getter task run for the remembered task
    Then the command succeeds
    And I remember the install handoff id
    When I run getter task install-result "requested" for the remembered handoff
    Then the command fails with a CLI usage error

  Scenario: User cannot poll task events with a zero limit
    Given an initialized getter data directory
    When I run getter task events after 0 limit 0
    Then the command fails with a CLI usage error

  Scenario: User receives structured errors for malformed task requests
    Given an initialized getter data directory
    And a malformed offline download request
    When I run getter task submit for that request
    Then the command fails with a download task error
